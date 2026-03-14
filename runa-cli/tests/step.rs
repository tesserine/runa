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

#[test]
fn step_dry_run_json_reports_ready_execution_plan_and_full_skill_status() {
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
        r#"{"title":"ship step"}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("prior-art/survey-1.json"),
        r#"{"source":"notes"}"#,
    )
    .unwrap();

    let output = runa_bin()
        .arg("step")
        .arg("--dry-run")
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
    assert!(value.get("cycle").is_none(), "{value:#}");

    let execution_plan = value["execution_plan"].as_array().unwrap();
    assert_eq!(execution_plan.len(), 1, "{value:#}");
    assert_eq!(execution_plan[0]["skill"], "implement");
    assert_eq!(execution_plan[0]["trigger"], "on_artifact(constraints)");
    assert_eq!(execution_plan[0]["context"]["skill"], "implement");
    assert_eq!(
        execution_plan[0]["context"]["expected_outputs"],
        serde_json::json!({
            "produces": ["implementation"],
            "may_produce": []
        })
    );
    assert_eq!(
        execution_plan[0]["context"]["inputs"],
        serde_json::json!([
            {
                "artifact_type": "constraints",
                "instance_id": "spec-1",
                "path": workspace.join("constraints/spec-1.json"),
                "content_hash": "sha256:dd4077b358533c789242e86ac7f5e7dffa0a587d5b4acfd343c612ae9ddfd315",
                "relationship": "requires"
            },
            {
                "artifact_type": "prior-art",
                "instance_id": "survey-1",
                "path": workspace.join("prior-art/survey-1.json"),
                "content_hash": "sha256:07de5216ca2c3ee50838fd24a2032bc4a9d77e73ba1de36a1cbdcd56b666946a",
                "relationship": "accepts"
            }
        ])
    );

    let skills = value["skills"].as_array().unwrap();
    assert_eq!(skills.len(), 3, "{value:#}");
    assert_eq!(skills[0]["name"], "implement");
    assert_eq!(skills[0]["status"], "ready");
    assert_eq!(skills[1]["name"], "verify");
    assert_eq!(skills[1]["status"], "blocked");
    assert_eq!(skills[2]["name"], "ground");
    assert_eq!(skills[2]["status"], "waiting");
    assert_eq!(
        skills[2]["unsatisfied_conditions"],
        serde_json::json!(["on_signal(begin): signal 'begin' is not active"])
    );
}

#[test]
fn step_dry_run_text_reports_why_when_no_skills_are_ready() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(&manifest_path, manifest_toml()).unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let output = runa_bin()
        .arg("step")
        .arg("--dry-run")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Execution plan: none"), "stdout: {stdout}");
    assert!(stdout.contains("No READY skills."), "stdout: {stdout}");
    assert!(stdout.contains("WAITING:"), "stdout: {stdout}");
    assert!(
        stdout.contains(
            "on_artifact(constraints): no valid instances of artifact type 'constraints' exist"
        ),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("on_signal(begin): signal 'begin' is not active"),
        "stdout: {stdout}"
    );
}

#[test]
fn step_without_dry_run_reports_placeholder_and_exits_non_zero() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(&manifest_path, manifest_toml()).unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let output = runa_bin()
        .arg("step")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "step should fail without --dry-run"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stderr.trim(),
        "error: Agent execution is not yet implemented. Use --dry-run to see the execution plan."
    );
}

#[test]
fn step_dry_run_reports_blocked_reasons_when_no_skills_are_ready() {
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
        r#"{"title":"ship step"}"#,
    )
    .unwrap();

    let output = runa_bin()
        .arg("step")
        .arg("--dry-run")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Execution plan: none"), "stdout: {stdout}");
    assert!(stdout.contains("No READY skills."), "stdout: {stdout}");
    assert!(stdout.contains("BLOCKED:"), "stdout: {stdout}");
    assert!(
        stdout.contains("implementation (missing)"),
        "stdout: {stdout}"
    );
}

#[cfg(unix)]
#[test]
fn step_dry_run_omits_partially_scanned_accepted_inputs_from_context() {
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
        r#"{"title":"ship step"}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("prior-art/visible.json"),
        r#"{"source":"notes"}"#,
    )
    .unwrap();
    let unreadable = workspace.join("prior-art/hidden.json");
    fs::write(&unreadable, r#"{"source":"hidden"}"#).unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o0)).unwrap();

    let output = runa_bin()
        .arg("step")
        .arg("--dry-run")
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

    let execution_plan = value["execution_plan"].as_array().unwrap();
    assert_eq!(execution_plan.len(), 1, "{value:#}");
    assert_eq!(
        execution_plan[0]["context"]["inputs"],
        serde_json::json!([
            {
                "artifact_type": "constraints",
                "instance_id": "spec-1",
                "path": workspace.join("constraints/spec-1.json"),
                "content_hash": "sha256:dd4077b358533c789242e86ac7f5e7dffa0a587d5b4acfd343c612ae9ddfd315",
                "relationship": "requires"
            }
        ])
    );

    let skills = value["skills"].as_array().unwrap();
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

#[test]
fn step_dry_run_json_reports_cycle_and_omits_execution_plan() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(
        &manifest_path,
        r#"
name = "groundwork"

[[artifact_types]]
name = "a"
schema = { type = "object", required = ["title"], properties = { title = { type = "string" } } }

[[artifact_types]]
name = "b"
schema = { type = "object", required = ["title"], properties = { title = { type = "string" } } }

[[skills]]
name = "first"
requires = ["b"]
produces = ["a"]
trigger = { type = "on_signal", name = "go" }

[[skills]]
name = "second"
requires = ["a"]
produces = ["b"]
trigger = { type = "on_signal", name = "go" }
"#,
    )
    .unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let output = runa_bin()
        .arg("step")
        .arg("--dry-run")
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
    assert_eq!(
        value["cycle"],
        serde_json::json!(["first", "second"]),
        "{value:#}"
    );
    assert_eq!(value["execution_plan"], serde_json::json!([]));
    let skills = value["skills"].as_array().unwrap();
    assert_eq!(skills.len(), 2, "{value:#}");
    assert_eq!(skills[0]["name"], "first");
    assert_eq!(skills[1]["name"], "second");
}

#[test]
fn step_dry_run_text_reports_cycle_and_no_execution_plan() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(
        &manifest_path,
        r#"
name = "groundwork"

[[artifact_types]]
name = "a"
schema = { type = "object", required = ["title"], properties = { title = { type = "string" } } }

[[artifact_types]]
name = "b"
schema = { type = "object", required = ["title"], properties = { title = { type = "string" } } }

[[skills]]
name = "first"
requires = ["b"]
produces = ["a"]
trigger = { type = "on_signal", name = "go" }

[[skills]]
name = "second"
requires = ["a"]
produces = ["b"]
trigger = { type = "on_signal", name = "go" }
"#,
    )
    .unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let output = runa_bin()
        .arg("step")
        .arg("--dry-run")
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
        stdout.contains("warning: dependency cycle detected: first -> second"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("Execution plan: none"), "stdout: {stdout}");
    assert!(stdout.contains("READY:"), "stdout: {stdout}");
}

#[test]
fn step_dry_run_keeps_non_cyclic_ready_skills_in_plan_when_cycle_exists() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(
        &manifest_path,
        r#"
name = "groundwork"

[[artifact_types]]
name = "seed"
schema = { type = "object", required = ["title"], properties = { title = { type = "string" } } }

[[artifact_types]]
name = "a"
schema = { type = "object", required = ["title"], properties = { title = { type = "string" } } }

[[artifact_types]]
name = "b"
schema = { type = "object", required = ["title"], properties = { title = { type = "string" } } }

[[artifact_types]]
name = "result"
schema = { type = "object", required = ["done"], properties = { done = { type = "boolean" } } }

[[skills]]
name = "independent"
requires = ["seed"]
produces = ["result"]
trigger = { type = "on_artifact", name = "seed" }

[[skills]]
name = "first"
requires = ["b"]
produces = ["a"]
trigger = { type = "on_signal", name = "go" }

[[skills]]
name = "second"
requires = ["a"]
produces = ["b"]
trigger = { type = "on_signal", name = "go" }
"#,
    )
    .unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("seed")).unwrap();
    fs::write(workspace.join("seed/input.json"), r#"{"title":"ship"}"#).unwrap();

    let output = runa_bin()
        .arg("step")
        .arg("--dry-run")
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
    assert_eq!(
        value["cycle"],
        serde_json::json!(["first", "second"]),
        "{value:#}"
    );

    let execution_plan = value["execution_plan"].as_array().unwrap();
    assert_eq!(execution_plan.len(), 1, "{value:#}");
    assert_eq!(execution_plan[0]["skill"], "independent");

    let skills = value["skills"].as_array().unwrap();
    assert_eq!(skills.len(), 3, "{value:#}");
    assert_eq!(skills[0]["name"], "independent");
}

#[test]
fn step_dry_run_keeps_ready_skills_downstream_of_cycle_when_inputs_exist() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(
        &manifest_path,
        r#"
name = "groundwork"

[[artifact_types]]
name = "a"
schema = { type = "object", required = ["title"], properties = { title = { type = "string" } } }

[[artifact_types]]
name = "b"
schema = { type = "object", required = ["title"], properties = { title = { type = "string" } } }

[[artifact_types]]
name = "result"
schema = { type = "object", required = ["done"], properties = { done = { type = "boolean" } } }

[[skills]]
name = "first"
requires = ["b"]
produces = ["a"]
trigger = { type = "on_signal", name = "go" }

[[skills]]
name = "second"
requires = ["a"]
produces = ["b"]
trigger = { type = "on_signal", name = "go" }

[[skills]]
name = "publish"
requires = ["a"]
produces = ["result"]
trigger = { type = "on_artifact", name = "a" }
"#,
    )
    .unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("a")).unwrap();
    fs::write(workspace.join("a/input.json"), r#"{"title":"already here"}"#).unwrap();

    let output = runa_bin()
        .arg("step")
        .arg("--dry-run")
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
    assert_eq!(value["cycle"], serde_json::json!(["first", "second"]), "{value:#}");

    let execution_plan = value["execution_plan"].as_array().unwrap();
    assert_eq!(execution_plan.len(), 1, "{value:#}");
    assert_eq!(execution_plan[0]["skill"], "publish");

    let skills = value["skills"].as_array().unwrap();
    assert_eq!(skills[0]["name"], "publish");
    assert_eq!(skills[0]["status"], "ready");
}

#[test]
fn step_dry_run_preserves_dependency_order_for_ready_skills_with_unrelated_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(
        &manifest_path,
        r#"
name = "groundwork"

[[artifact_types]]
name = "root"
schema = { type = "object", required = ["title"], properties = { title = { type = "string" } } }

[[artifact_types]]
name = "seed"
schema = { type = "object", required = ["title"], properties = { title = { type = "string" } } }

[[artifact_types]]
name = "result"
schema = { type = "object", required = ["done"], properties = { done = { type = "boolean" } } }

[[artifact_types]]
name = "a"
schema = { type = "object", required = ["title"], properties = { title = { type = "string" } } }

[[artifact_types]]
name = "b"
schema = { type = "object", required = ["title"], properties = { title = { type = "string" } } }

[[skills]]
name = "independent"
requires = ["root"]
produces = ["seed"]
trigger = { type = "on_artifact", name = "root" }

[[skills]]
name = "publish"
requires = ["seed"]
produces = ["result"]
trigger = { type = "on_artifact", name = "seed" }

[[skills]]
name = "first"
requires = ["b"]
produces = ["a"]
trigger = { type = "on_signal", name = "go" }

[[skills]]
name = "second"
requires = ["a"]
produces = ["b"]
trigger = { type = "on_signal", name = "go" }
"#,
    )
    .unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("root")).unwrap();
    fs::create_dir_all(workspace.join("seed")).unwrap();
    fs::write(workspace.join("root/input.json"), r#"{"title":"root"}"#).unwrap();
    fs::write(workspace.join("seed/input.json"), r#"{"title":"ship"}"#).unwrap();

    let output = runa_bin()
        .arg("step")
        .arg("--dry-run")
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
    let execution_plan = value["execution_plan"].as_array().unwrap();
    assert_eq!(execution_plan.len(), 2, "{value:#}");
    assert_eq!(execution_plan[0]["skill"], "independent");
    assert_eq!(execution_plan[1]["skill"], "publish");
}
