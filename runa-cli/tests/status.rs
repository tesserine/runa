use std::fs;
use std::process::Command;

fn runa_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_runa"))
}

fn manifest_toml() -> &'static str {
    r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"
schema = { type = "object", required = ["title"], properties = { title = { type = "string" } } }

[[artifact_types]]
name = "prior-art"
schema = { type = "object", required = ["source"], properties = { source = { type = "string" } } }

[[artifact_types]]
name = "implementation"
schema = { type = "object", required = ["done"], properties = { done = { type = "boolean" } } }

[[skills]]
name = "implement"
requires = ["constraints"]
accepts = ["prior-art"]
produces = ["implementation"]
trigger = { type = "on_artifact", name = "constraints" }

[[skills]]
name = "verify"
requires = ["implementation"]
trigger = { type = "on_artifact", name = "constraints" }

[[skills]]
name = "ground"
trigger = { type = "on_signal", name = "begin" }
"#
}

fn init_project(project_dir: &std::path::Path, manifest_path: &std::path::Path) {
    let output = runa_bin()
        .arg("init")
        .arg("--methodology")
        .arg(manifest_path)
        .current_dir(project_dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn scan_project(project_dir: &std::path::Path) {
    let output = runa_bin()
        .arg("scan")
        .current_dir(project_dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "scan failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn status_groups_ready_blocked_and_waiting_after_implicit_scan() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(&manifest_path, manifest_toml()).unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::create_dir_all(workspace.join("prior-art")).unwrap();
    fs::write(
        workspace.join("constraints/spec-1.json"),
        r#"{"title":"ship status"}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("prior-art/survey-1.json"),
        r#"{"source":"notes"}"#,
    )
    .unwrap();

    let output = runa_bin()
        .arg("status")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Methodology: groundwork"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("READY:"), "stdout: {stdout}");
    assert!(stdout.contains("BLOCKED:"), "stdout: {stdout}");
    assert!(stdout.contains("WAITING:"), "stdout: {stdout}");

    let ready_pos = stdout.find("READY:").unwrap();
    let blocked_pos = stdout.find("BLOCKED:").unwrap();
    let waiting_pos = stdout.find("WAITING:").unwrap();
    assert!(ready_pos < blocked_pos, "stdout: {stdout}");
    assert!(blocked_pos < waiting_pos, "stdout: {stdout}");

    let implement_pos = stdout.find("  implement\n").unwrap();
    let verify_pos = stdout.find("  verify\n").unwrap();
    let ground_pos = stdout.find("  ground\n").unwrap();
    assert!(implement_pos < verify_pos, "stdout: {stdout}");
    assert!(verify_pos < ground_pos, "stdout: {stdout}");
}

#[test]
fn status_json_reports_ordered_skills_and_status_specific_fields() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(&manifest_path, manifest_toml()).unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::create_dir_all(workspace.join("prior-art")).unwrap();
    fs::write(
        workspace.join("constraints/spec-1.json"),
        r#"{"title":"ship status"}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("prior-art/survey-1.json"),
        r#"{"source":"notes"}"#,
    )
    .unwrap();

    let output = runa_bin()
        .arg("status")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["version"], 1);
    assert_eq!(value["methodology"], "groundwork");
    assert_eq!(value["scan_warnings"], serde_json::json!([]));

    let skills = value["skills"].as_array().unwrap();
    assert_eq!(skills.len(), 3, "{value:#}");

    assert_eq!(skills[0]["name"], "implement");
    assert_eq!(skills[0]["status"], "ready");
    assert_eq!(skills[0]["trigger"], "satisfied");
    assert_eq!(
        skills[0]["inputs"],
        serde_json::json!([
            {
                "artifact_type": "constraints",
                "instance_id": "spec-1",
                "path": ".runa/workspace/constraints/spec-1.json",
                "relationship": "requires"
            },
            {
                "artifact_type": "prior-art",
                "instance_id": "survey-1",
                "path": ".runa/workspace/prior-art/survey-1.json",
                "relationship": "accepts"
            }
        ])
    );
    assert!(skills[0].get("precondition_failures").is_none());
    assert!(skills[0].get("unsatisfied_conditions").is_none());

    assert_eq!(skills[1]["name"], "verify");
    assert_eq!(skills[1]["status"], "blocked");
    assert_eq!(skills[1]["trigger"], "satisfied");
    assert_eq!(
        skills[1]["precondition_failures"],
        serde_json::json!([
            {
                "artifact_type": "implementation",
                "reason": "missing"
            }
        ])
    );
    assert!(skills[1].get("inputs").is_none());
    assert!(skills[1].get("unsatisfied_conditions").is_none());

    assert_eq!(skills[2]["name"], "ground");
    assert_eq!(skills[2]["status"], "waiting");
    assert_eq!(skills[2]["trigger"], "not_satisfied");
    assert_eq!(
        skills[2]["unsatisfied_conditions"],
        serde_json::json!(["on_signal(begin): signal 'begin' is not active"])
    );
    assert!(skills[2].get("inputs").is_none());
    assert!(skills[2].get("precondition_failures").is_none());
}

#[test]
fn status_json_reports_stale_failures_and_composite_waiting_conditions() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(
        &manifest_path,
        r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"
schema = { type = "object", required = ["title"], properties = { title = { type = "string" } } }

[[artifact_types]]
name = "implementation"
schema = { type = "object", required = ["done"], properties = { done = { type = "boolean" } } }

[[skills]]
name = "verify"
requires = ["implementation"]
trigger = { type = "on_artifact", name = "constraints" }

[[skills]]
name = "release"
trigger = { type = "all_of", conditions = [
    { type = "on_signal", name = "approve" },
    { type = "any_of", conditions = [
        { type = "on_artifact", name = "implementation" },
        { type = "on_signal", name = "override" }
    ] }
] }
"#,
    )
    .unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::create_dir_all(workspace.join("implementation")).unwrap();
    fs::write(
        workspace.join("constraints/spec-1.json"),
        r#"{"title":"ship status"}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("implementation/impl-1.json"),
        r#"{"done":true}"#,
    )
    .unwrap();

    scan_project(&project_dir);

    let store_path = project_dir.join(".runa/store/implementation/impl-1.json");
    let mut state: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&store_path).unwrap()).unwrap();
    state["status"] = serde_json::json!("stale");
    fs::write(&store_path, serde_json::to_string_pretty(&state).unwrap()).unwrap();

    let output = runa_bin()
        .arg("status")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let skills = value["skills"].as_array().unwrap();
    assert_eq!(skills.len(), 2, "{value:#}");
    assert_eq!(value["scan_warnings"], serde_json::json!([]));

    assert_eq!(skills[0]["name"], "verify");
    assert_eq!(skills[0]["status"], "blocked");
    assert_eq!(
        skills[0]["precondition_failures"],
        serde_json::json!([
            {
                "artifact_type": "implementation",
                "reason": "stale"
            }
        ])
    );

    assert_eq!(skills[1]["name"], "release");
    assert_eq!(skills[1]["status"], "waiting");
    assert_eq!(
        skills[1]["unsatisfied_conditions"],
        serde_json::json!([
            "on_signal(approve): signal 'approve' is not active",
            "on_artifact(implementation): artifact type 'implementation' has invalid or stale instances",
            "on_signal(override): signal 'override' is not active"
        ])
    );
}

#[test]
fn status_errors_on_uninitialized_project() {
    let dir = tempfile::tempdir().unwrap();

    let output = runa_bin()
        .arg("status")
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(!output.status.success(), "status should fail without init");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no config found"), "stderr: {stderr}");
}

#[cfg(unix)]
#[test]
fn status_keeps_skills_ready_when_only_accepted_types_are_partially_scanned() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(&manifest_path, manifest_toml()).unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::create_dir_all(workspace.join("prior-art")).unwrap();
    fs::write(
        workspace.join("constraints/spec-1.json"),
        r#"{"title":"ship status"}"#,
    )
    .unwrap();
    let unreadable = workspace.join("prior-art/survey-1.json");
    fs::write(&unreadable, r#"{"source":"notes"}"#).unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o0)).unwrap();

    let output = runa_bin()
        .arg("status")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o644)).unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["scan_warnings"],
        serde_json::json!([
            "artifact type 'prior-art' was only partially scanned: 1 unreadable entry"
        ])
    );

    let skills = value["skills"].as_array().unwrap();
    assert_eq!(skills[0]["name"], "implement");
    assert_eq!(skills[0]["status"], "ready");
    assert_eq!(
        skills[0]["inputs"],
        serde_json::json!([
            {
                "artifact_type": "constraints",
                "instance_id": "spec-1",
                "path": ".runa/workspace/constraints/spec-1.json",
                "relationship": "requires"
            }
        ])
    );
}

#[cfg(unix)]
#[test]
fn status_blocks_skills_with_partial_required_types_and_reports_scan_warnings() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(&manifest_path, manifest_toml()).unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::create_dir_all(workspace.join("prior-art")).unwrap();
    fs::write(
        workspace.join("constraints/spec-1.json"),
        r#"{"title":"ship status"}"#,
    )
    .unwrap();
    let unreadable = workspace.join("constraints/spec-2.json");
    fs::write(&unreadable, r#"{"title":"hidden"}"#).unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o0)).unwrap();
    fs::write(
        workspace.join("prior-art/survey-1.json"),
        r#"{"source":"notes"}"#,
    )
    .unwrap();

    let json_output = runa_bin()
        .arg("status")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    let text_output = runa_bin()
        .arg("status")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o644)).unwrap();

    assert!(
        json_output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&json_output.stderr)
    );

    let value: serde_json::Value = serde_json::from_slice(&json_output.stdout).unwrap();
    assert_eq!(
        value["scan_warnings"],
        serde_json::json!([
            "artifact type 'constraints' was only partially scanned: 1 unreadable entry"
        ])
    );

    let skills = value["skills"].as_array().unwrap();
    assert_eq!(skills[0]["name"], "implement");
    assert_eq!(skills[0]["status"], "blocked");
    assert_eq!(skills[0]["trigger"], "satisfied");
    assert_eq!(
        skills[0]["precondition_failures"],
        serde_json::json!([
            {
                "artifact_type": "constraints",
                "reason": "scan_incomplete"
            }
        ])
    );
    assert!(skills[0].get("inputs").is_none());

    assert!(
        text_output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&text_output.stderr)
    );

    let stdout = String::from_utf8_lossy(&text_output.stdout);
    assert!(stdout.contains("Scan warnings:"), "stdout: {stdout}");
    assert!(
        stdout
            .contains("artifact type 'constraints' was only partially scanned: 1 unreadable entry"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("constraints (scan_incomplete)"),
        "stdout: {stdout}"
    );
}

#[cfg(unix)]
#[test]
fn status_blocks_skills_when_partial_scan_affects_requires_even_if_trigger_is_unsatisfied() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(
        &manifest_path,
        r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"
schema = { type = "object", required = ["title"], properties = { title = { type = "string" } } }

[[skills]]
name = "implement"
requires = ["constraints"]
trigger = { type = "on_signal", name = "begin" }
"#,
    )
    .unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::write(
        workspace.join("constraints/spec-1.json"),
        r#"{"title":"ship status"}"#,
    )
    .unwrap();
    let unreadable = workspace.join("constraints/spec-2.json");
    fs::write(&unreadable, r#"{"title":"hidden"}"#).unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o0)).unwrap();

    let output = runa_bin()
        .arg("status")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o644)).unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let skills = value["skills"].as_array().unwrap();
    assert_eq!(skills[0]["name"], "implement");
    assert_eq!(skills[0]["status"], "blocked");
    assert_eq!(skills[0]["trigger"], "not_satisfied");
    assert_eq!(
        skills[0]["precondition_failures"],
        serde_json::json!([
            {
                "artifact_type": "constraints",
                "reason": "scan_incomplete"
            }
        ])
    );
    assert!(skills[0].get("unsatisfied_conditions").is_none());
}

#[cfg(unix)]
#[test]
fn status_reports_all_partial_required_types_as_scan_incomplete_failures() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(
        &manifest_path,
        r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"
schema = { type = "object", required = ["title"], properties = { title = { type = "string" } } }

[[artifact_types]]
name = "implementation"
schema = { type = "object", required = ["done"], properties = { done = { type = "boolean" } } }

[[skills]]
name = "verify"
requires = ["constraints", "implementation"]
trigger = { type = "on_artifact", name = "constraints" }
"#,
    )
    .unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::create_dir_all(workspace.join("implementation")).unwrap();
    fs::write(
        workspace.join("constraints/spec-1.json"),
        r#"{"title":"ship status"}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("implementation/impl-1.json"),
        r#"{"done":true}"#,
    )
    .unwrap();
    let unreadable_constraints = workspace.join("constraints/spec-2.json");
    fs::write(&unreadable_constraints, r#"{"title":"hidden"}"#).unwrap();
    fs::set_permissions(&unreadable_constraints, fs::Permissions::from_mode(0o0)).unwrap();
    let unreadable_implementation = workspace.join("implementation/impl-2.json");
    fs::write(&unreadable_implementation, r#"{"done":false}"#).unwrap();
    fs::set_permissions(&unreadable_implementation, fs::Permissions::from_mode(0o0)).unwrap();

    let output = runa_bin()
        .arg("status")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    fs::set_permissions(&unreadable_constraints, fs::Permissions::from_mode(0o644)).unwrap();
    fs::set_permissions(
        &unreadable_implementation,
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let skills = value["skills"].as_array().unwrap();
    assert_eq!(skills[0]["name"], "verify");
    assert_eq!(skills[0]["status"], "blocked");
    assert_eq!(
        skills[0]["precondition_failures"],
        serde_json::json!([
            {
                "artifact_type": "constraints",
                "reason": "scan_incomplete"
            },
            {
                "artifact_type": "implementation",
                "reason": "scan_incomplete"
            }
        ])
    );
}

#[cfg(unix)]
#[test]
fn status_blocks_skills_when_partial_scan_affects_trigger_only_artifact_types() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(
        &manifest_path,
        r#"
name = "groundwork"

[[artifact_types]]
name = "report"
schema = { type = "object", required = ["title"], properties = { title = { type = "string" } } }

[[skills]]
name = "repair"
trigger = { type = "on_artifact", name = "report" }
"#,
    )
    .unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("report")).unwrap();
    fs::write(
        workspace.join("report/visible.json"),
        r#"{"title":"visible"}"#,
    )
    .unwrap();
    let unreadable = workspace.join("report/hidden.json");
    fs::write(&unreadable, r#"{"title":"hidden"}"#).unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o0)).unwrap();

    let output = runa_bin()
        .arg("status")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o644)).unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let skills = value["skills"].as_array().unwrap();
    assert_eq!(skills[0]["name"], "repair");
    assert_eq!(skills[0]["status"], "blocked");
    assert_eq!(skills[0]["trigger"], "satisfied");
    assert_eq!(
        skills[0]["precondition_failures"],
        serde_json::json!([
            {
                "artifact_type": "report",
                "reason": "scan_incomplete"
            }
        ])
    );
}

#[test]
fn status_preserves_reason_for_empty_any_of_triggers() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(
        &manifest_path,
        r#"
name = "groundwork"

artifact_types = []

[[skills]]
name = "impossible"
trigger = { type = "any_of", conditions = [] }
"#,
    )
    .unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let json_output = runa_bin()
        .arg("status")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        json_output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&json_output.stderr)
    );

    let value: serde_json::Value = serde_json::from_slice(&json_output.stdout).unwrap();
    let skills = value["skills"].as_array().unwrap();
    assert_eq!(skills[0]["name"], "impossible");
    assert_eq!(skills[0]["status"], "waiting");
    assert_eq!(
        skills[0]["unsatisfied_conditions"],
        serde_json::json!(["any_of(): any_of with no conditions"])
    );

    let text_output = runa_bin()
        .arg("status")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        text_output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&text_output.stderr)
    );

    let stdout = String::from_utf8_lossy(&text_output.stdout);
    assert!(
        stdout.contains("any_of(): any_of with no conditions"),
        "stdout: {stdout}"
    );
}

#[cfg(unix)]
#[test]
fn status_keeps_on_invalid_ready_when_partial_scan_cannot_change_satisfied_trigger() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(
        &manifest_path,
        r#"
name = "groundwork"

[[artifact_types]]
name = "report"
schema = { type = "object", required = ["title"], properties = { title = { type = "string" } } }

[[skills]]
name = "repair"
trigger = { type = "on_invalid", name = "report" }
"#,
    )
    .unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("report")).unwrap();
    fs::write(workspace.join("report/visible.json"), r#"{"bad":true}"#).unwrap();
    let unreadable = workspace.join("report/hidden.json");
    fs::write(&unreadable, r#"{"title":"hidden"}"#).unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o0)).unwrap();

    let output = runa_bin()
        .arg("status")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o644)).unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let skills = value["skills"].as_array().unwrap();
    assert_eq!(skills[0]["name"], "repair");
    assert_eq!(skills[0]["status"], "ready");
    assert_eq!(skills[0]["trigger"], "satisfied");
    assert!(skills[0].get("precondition_failures").is_none());
}

#[cfg(unix)]
#[test]
fn status_keeps_any_of_ready_when_other_branch_proves_satisfaction() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(
        &manifest_path,
        r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"
schema = { type = "object", required = ["title"], properties = { title = { type = "string" } } }

[[artifact_types]]
name = "report"
schema = { type = "object", required = ["title"], properties = { title = { type = "string" } } }

[[skills]]
name = "implement"
trigger = { type = "any_of", conditions = [
    { type = "on_invalid", name = "constraints" },
    { type = "on_artifact", name = "report" }
] }
"#,
    )
    .unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::create_dir_all(workspace.join("report")).unwrap();
    fs::write(workspace.join("constraints/bad.json"), r#"{"bad":true}"#).unwrap();
    let unreadable = workspace.join("report/hidden.json");
    fs::write(&unreadable, r#"{"title":"hidden"}"#).unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o0)).unwrap();

    let output = runa_bin()
        .arg("status")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o644)).unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let skills = value["skills"].as_array().unwrap();
    assert_eq!(skills[0]["name"], "implement");
    assert_eq!(skills[0]["status"], "ready");
    assert_eq!(skills[0]["trigger"], "satisfied");
}

#[cfg(unix)]
#[test]
fn status_blocks_on_artifact_when_only_unreadable_instances_exist() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(
        &manifest_path,
        r#"
name = "groundwork"

[[artifact_types]]
name = "report"
schema = { type = "object", required = ["title"], properties = { title = { type = "string" } } }

[[skills]]
name = "publish"
trigger = { type = "on_artifact", name = "report" }
"#,
    )
    .unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("report")).unwrap();
    let unreadable = workspace.join("report/hidden.json");
    fs::write(&unreadable, r#"{"title":"hidden"}"#).unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o0)).unwrap();

    let output = runa_bin()
        .arg("status")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o644)).unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let skills = value["skills"].as_array().unwrap();
    assert_eq!(skills[0]["name"], "publish");
    assert_eq!(skills[0]["status"], "blocked");
    assert_eq!(skills[0]["trigger"], "not_satisfied");
    assert_eq!(
        skills[0]["precondition_failures"],
        serde_json::json!([
            {
                "artifact_type": "report",
                "reason": "scan_incomplete"
            }
        ])
    );
}

#[cfg(unix)]
#[test]
fn status_blocks_untrustworthy_not_satisfied_on_invalid_and_on_change_triggers() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(
        &manifest_path,
        r#"
name = "groundwork"

[[artifact_types]]
name = "report"
schema = { type = "object", required = ["title"], properties = { title = { type = "string" } } }

[[artifact_types]]
name = "doc"
schema = { type = "object", required = ["title"], properties = { title = { type = "string" } } }

[[skills]]
name = "repair"
trigger = { type = "on_invalid", name = "report" }

[[skills]]
name = "review"
trigger = { type = "on_change", name = "doc" }
"#,
    )
    .unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("report")).unwrap();
    fs::create_dir_all(workspace.join("doc")).unwrap();
    let unreadable_report = workspace.join("report/hidden.json");
    fs::write(&unreadable_report, r#"{"bad":true}"#).unwrap();
    fs::set_permissions(&unreadable_report, fs::Permissions::from_mode(0o0)).unwrap();
    let unreadable_doc = workspace.join("doc/hidden.json");
    fs::write(&unreadable_doc, r#"{"title":"hidden"}"#).unwrap();
    fs::set_permissions(&unreadable_doc, fs::Permissions::from_mode(0o0)).unwrap();

    let output = runa_bin()
        .arg("status")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    fs::set_permissions(&unreadable_report, fs::Permissions::from_mode(0o644)).unwrap();
    fs::set_permissions(&unreadable_doc, fs::Permissions::from_mode(0o644)).unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let skills = value["skills"].as_array().unwrap();
    let repair = skills
        .iter()
        .find(|skill| skill["name"] == "repair")
        .unwrap();
    let review = skills
        .iter()
        .find(|skill| skill["name"] == "review")
        .unwrap();

    assert_eq!(repair["status"], "blocked");
    assert_eq!(repair["trigger"], "not_satisfied");
    assert_eq!(
        repair["precondition_failures"],
        serde_json::json!([
            {
                "artifact_type": "report",
                "reason": "scan_incomplete"
            }
        ])
    );

    assert_eq!(review["status"], "blocked");
    assert_eq!(review["trigger"], "not_satisfied");
    assert_eq!(
        review["precondition_failures"],
        serde_json::json!([
            {
                "artifact_type": "doc",
                "reason": "scan_incomplete"
            }
        ])
    );
}

#[cfg(unix)]
#[test]
fn status_preserves_invalid_preconditions_alongside_scan_incomplete() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(
        &manifest_path,
        r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"
schema = { type = "object", required = ["title"], properties = { title = { type = "string" } } }

[[skills]]
name = "implement"
requires = ["constraints"]
trigger = { type = "on_signal", name = "begin" }
"#,
    )
    .unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::write(workspace.join("constraints/bad.json"), r#"{"bad":true}"#).unwrap();
    let unreadable = workspace.join("constraints/hidden.json");
    fs::write(&unreadable, r#"{"title":"hidden"}"#).unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o0)).unwrap();

    let output = runa_bin()
        .arg("status")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o644)).unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let skills = value["skills"].as_array().unwrap();
    assert_eq!(skills[0]["name"], "implement");
    assert_eq!(skills[0]["status"], "blocked");
    assert_eq!(skills[0]["trigger"], "not_satisfied");
    assert_eq!(
        skills[0]["precondition_failures"],
        serde_json::json!([
            {
                "artifact_type": "constraints",
                "reason": "scan_incomplete"
            },
            {
                "artifact_type": "constraints",
                "reason": "invalid"
            }
        ])
    );
}

#[cfg(unix)]
#[test]
fn status_keeps_on_artifact_waiting_when_visible_invalid_instance_makes_it_definitely_false() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(
        &manifest_path,
        r#"
name = "groundwork"

[[artifact_types]]
name = "report"
schema = { type = "object", required = ["title"], properties = { title = { type = "string" } } }

[[skills]]
name = "publish"
trigger = { type = "on_artifact", name = "report" }
"#,
    )
    .unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("report")).unwrap();
    fs::write(workspace.join("report/bad.json"), r#"{"bad":true}"#).unwrap();
    let unreadable = workspace.join("report/hidden.json");
    fs::write(&unreadable, r#"{"title":"hidden"}"#).unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o0)).unwrap();

    let output = runa_bin()
        .arg("status")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o644)).unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let skills = value["skills"].as_array().unwrap();
    assert_eq!(skills[0]["name"], "publish");
    assert_eq!(skills[0]["status"], "waiting");
    assert_eq!(skills[0]["trigger"], "not_satisfied");
    assert_eq!(
        skills[0]["unsatisfied_conditions"],
        serde_json::json!([
            "on_artifact(report): artifact type 'report' has invalid or stale instances"
        ])
    );
    assert!(skills[0].get("precondition_failures").is_none());
}
