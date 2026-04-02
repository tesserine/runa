mod common;

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

[[artifact_types]]
name = "prior-art"

[[artifact_types]]
name = "implementation"

[[protocols]]
name = "implement"
requires = ["constraints"]
accepts = ["prior-art"]
produces = ["implementation"]
trigger = { type = "on_artifact", name = "constraints" }

[[protocols]]
name = "verify"
requires = ["implementation"]
trigger = { type = "on_artifact", name = "constraints" }

[[protocols]]
name = "ground"
trigger = { type = "on_invalid", name = "implementation" }
"#
}

fn manifest_toml_schemas() -> &'static [(&'static str, &'static str)] {
    &[
        (
            "constraints",
            r#"{"type": "object", "required": ["title"], "properties": {"title": {"type": "string"}}}"#,
        ),
        (
            "prior-art",
            r#"{"type": "object", "required": ["source"], "properties": {"source": {"type": "string"}}}"#,
        ),
        (
            "implementation",
            r#"{"type": "object", "required": ["done"], "properties": {"done": {"type": "boolean"}}}"#,
        ),
    ]
}

fn manifest_toml_protocols() -> &'static [&'static str] {
    &["implement", "verify", "ground"]
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
fn state_filters_protocols_by_declared_scope() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"

[[artifact_types]]
name = "implementation"

[[protocols]]
name = "ground"
produces = ["constraints"]
trigger = { type = "on_artifact", name = "constraints" }

[[protocols]]
name = "implement"
requires = ["constraints"]
produces = ["implementation"]
scoped = true
trigger = { type = "on_artifact", name = "constraints" }
"#,
        &[
            (
                "constraints",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            (
                "implementation",
                r#"{"type":"object","required":["done","work_unit"],"properties":{"done":{"type":"boolean"},"work_unit":{"type":"string"}}}"#,
            ),
        ],
        &["ground", "implement"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::write(
        workspace.join("constraints/spec-a.json"),
        r#"{"title":"ship state","work_unit":"wu-a"}"#,
    )
    .unwrap();

    let unscoped = runa_bin()
        .arg("state")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();
    assert!(
        unscoped.status.success(),
        "state failed: {}",
        String::from_utf8_lossy(&unscoped.stderr)
    );
    let unscoped_json: serde_json::Value = serde_json::from_slice(&unscoped.stdout).unwrap();
    let unscoped_protocols = unscoped_json["protocols"].as_array().unwrap();
    assert_eq!(unscoped_protocols.len(), 1);
    assert_eq!(unscoped_protocols[0]["name"], "ground");
    assert!(unscoped_protocols[0].get("work_unit").is_none());

    let scoped = runa_bin()
        .arg("state")
        .arg("--json")
        .arg("--work-unit")
        .arg("wu-a")
        .current_dir(&project_dir)
        .output()
        .unwrap();
    assert!(
        scoped.status.success(),
        "state --work-unit failed: {}",
        String::from_utf8_lossy(&scoped.stderr)
    );
    let scoped_json: serde_json::Value = serde_json::from_slice(&scoped.stdout).unwrap();
    let scoped_protocols = scoped_json["protocols"].as_array().unwrap();
    assert_eq!(scoped_protocols.len(), 1);
    assert_eq!(scoped_protocols[0]["name"], "implement");
    assert_eq!(scoped_protocols[0]["work_unit"], "wu-a");
}

#[test]
fn state_scoped_ignores_unscoped_cycle_participants() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "x"

[[artifact_types]]
name = "y"

[[protocols]]
name = "publish"
requires = ["y"]
produces = ["x"]
trigger = { type = "on_artifact", name = "y" }

[[protocols]]
name = "implement"
requires = ["x"]
produces = ["y"]
scoped = true
trigger = { type = "on_artifact", name = "x" }
"#,
        &[
            (
                "x",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            (
                "y",
                r#"{"type":"object","required":["title","work_unit"],"properties":{"title":{"type":"string"},"work_unit":{"type":"string"}}}"#,
            ),
        ],
        &["publish", "implement"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("x")).unwrap();
    fs::write(workspace.join("x/input.json"), r#"{"title":"ship"}"#).unwrap();

    let output = runa_bin()
        .arg("state")
        .arg("--json")
        .arg("--work-unit")
        .arg("wu-a")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let protocols = value["protocols"].as_array().unwrap();
    assert_eq!(protocols.len(), 1, "{value:#}");
    assert_eq!(protocols[0]["name"], "implement");
    assert_eq!(protocols[0]["status"], "ready");
    assert_eq!(protocols[0]["trigger"], "satisfied");
}

#[test]
fn state_unscoped_ignores_scoped_cycle_participants() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "seed"

[[artifact_types]]
name = "result"

[[artifact_types]]
name = "a"

[[artifact_types]]
name = "b"

[[protocols]]
name = "publish"
requires = ["seed"]
produces = ["result"]
trigger = { type = "on_artifact", name = "seed" }

[[protocols]]
name = "first"
requires = ["b"]
produces = ["a"]
scoped = true
trigger = { type = "on_artifact", name = "b" }

[[protocols]]
name = "second"
requires = ["a"]
produces = ["b"]
scoped = true
trigger = { type = "on_artifact", name = "a" }
"#,
        &[
            (
                "seed",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            (
                "result",
                r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#,
            ),
            (
                "a",
                r#"{"type":"object","required":["title","work_unit"],"properties":{"title":{"type":"string"},"work_unit":{"type":"string"}}}"#,
            ),
            (
                "b",
                r#"{"type":"object","required":["title","work_unit"],"properties":{"title":{"type":"string"},"work_unit":{"type":"string"}}}"#,
            ),
        ],
        &["publish", "first", "second"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("seed")).unwrap();
    fs::write(workspace.join("seed/input.json"), r#"{"title":"ship"}"#).unwrap();
    fs::create_dir_all(workspace.join("a")).unwrap();
    fs::create_dir_all(workspace.join("b")).unwrap();
    fs::write(
        workspace.join("a/current.json"),
        r#"{"title":"a","work_unit":"wu-a"}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("b/current.json"),
        r#"{"title":"b","work_unit":"wu-a"}"#,
    )
    .unwrap();

    let output = runa_bin()
        .arg("state")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let protocols = value["protocols"].as_array().unwrap();
    assert_eq!(protocols.len(), 1, "{value:#}");
    assert_eq!(protocols[0]["name"], "publish");
    assert_eq!(protocols[0]["status"], "ready");
    assert_eq!(protocols[0]["trigger"], "satisfied");
}

#[test]
fn state_scoped_cycle_participants_are_waiting_not_ready() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "seed-a"

[[artifact_types]]
name = "seed-b"

[[artifact_types]]
name = "a"

[[artifact_types]]
name = "b"

[[protocols]]
name = "first"
requires = ["b"]
produces = ["a"]
scoped = true
trigger = { type = "on_artifact", name = "seed-a" }

[[protocols]]
name = "second"
requires = ["a"]
produces = ["b"]
scoped = true
trigger = { type = "on_artifact", name = "seed-b" }
"#,
        &[
            (
                "seed-a",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            (
                "seed-b",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            (
                "a",
                r#"{"type":"object","required":["title","work_unit"],"properties":{"title":{"type":"string"},"work_unit":{"type":"string"}}}"#,
            ),
            (
                "b",
                r#"{"type":"object","required":["title","work_unit"],"properties":{"title":{"type":"string"},"work_unit":{"type":"string"}}}"#,
            ),
        ],
        &["first", "second"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("a")).unwrap();
    fs::create_dir_all(workspace.join("b")).unwrap();
    fs::create_dir_all(workspace.join("seed-a")).unwrap();
    fs::create_dir_all(workspace.join("seed-b")).unwrap();
    fs::write(
        workspace.join("a/current.json"),
        r#"{"title":"a","work_unit":"wu-a"}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("b/current.json"),
        r#"{"title":"b","work_unit":"wu-a"}"#,
    )
    .unwrap();
    scan_project(&project_dir);
    fs::write(workspace.join("seed-a/input.json"), r#"{"title":"seed-a"}"#).unwrap();
    fs::write(workspace.join("seed-b/input.json"), r#"{"title":"seed-b"}"#).unwrap();

    let output = runa_bin()
        .arg("state")
        .arg("--json")
        .arg("--work-unit")
        .arg("wu-a")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "warning: dependency cycle detected: first -> second\n"
    );

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let protocols = value["protocols"].as_array().unwrap();
    assert_eq!(protocols.len(), 2, "{value:#}");
    assert_eq!(protocols[0]["status"], "waiting");
    assert_eq!(protocols[0]["trigger"], "satisfied");
    assert_eq!(
        protocols[0]["unsatisfied_conditions"],
        serde_json::json!(["dependency cycle detected: first -> second"])
    );
    assert_eq!(protocols[1]["status"], "waiting");
    assert_eq!(protocols[1]["trigger"], "satisfied");
    assert_eq!(
        protocols[1]["unsatisfied_conditions"],
        serde_json::json!(["dependency cycle detected: first -> second"])
    );
}

#[test]
fn state_groups_ready_blocked_and_waiting_after_implicit_scan() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        manifest_toml(),
        manifest_toml_schemas(),
        manifest_toml_protocols(),
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::create_dir_all(workspace.join("prior-art")).unwrap();
    fs::write(
        workspace.join("constraints/spec-1.json"),
        r#"{"title":"ship state"}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("prior-art/survey-1.json"),
        r#"{"source":"notes"}"#,
    )
    .unwrap();

    let output = runa_bin()
        .arg("state")
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
fn state_json_reports_ordered_skills_and_status_specific_fields() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        manifest_toml(),
        manifest_toml_schemas(),
        manifest_toml_protocols(),
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::create_dir_all(workspace.join("prior-art")).unwrap();
    fs::write(
        workspace.join("constraints/spec-1.json"),
        r#"{"title":"ship state"}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("prior-art/survey-1.json"),
        r#"{"source":"notes"}"#,
    )
    .unwrap();

    let output = runa_bin()
        .arg("state")
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
    assert_eq!(value["version"], 2);
    assert_eq!(value["methodology"], "groundwork");
    assert_eq!(value["scan_warnings"], serde_json::json!([]));

    let protocols = value["protocols"].as_array().unwrap();
    assert_eq!(protocols.len(), 3, "{value:#}");

    assert_eq!(protocols[0]["name"], "implement");
    assert_eq!(protocols[0]["status"], "ready");
    assert_eq!(protocols[0]["trigger"], "satisfied");
    assert_eq!(
        protocols[0]["inputs"],
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
    assert!(protocols[0].get("precondition_failures").is_none());
    assert!(protocols[0].get("unsatisfied_conditions").is_none());

    assert_eq!(protocols[1]["name"], "verify");
    assert_eq!(protocols[1]["status"], "blocked");
    assert_eq!(protocols[1]["trigger"], "satisfied");
    assert_eq!(
        protocols[1]["precondition_failures"],
        serde_json::json!([
            {
                "artifact_type": "implementation",
                "reason": "missing"
            }
        ])
    );
    assert!(protocols[1].get("inputs").is_none());
    assert!(protocols[1].get("unsatisfied_conditions").is_none());

    assert_eq!(protocols[2]["name"], "ground");
    assert_eq!(protocols[2]["status"], "waiting");
    assert_eq!(protocols[2]["trigger"], "not_satisfied");
    assert_eq!(
        protocols[2]["unsatisfied_conditions"],
        serde_json::json!([
            "on_invalid(implementation): no invalid instances of artifact type 'implementation'"
        ])
    );
    assert!(protocols[2].get("inputs").is_none());
    assert!(protocols[2].get("precondition_failures").is_none());
}

#[test]
fn state_json_reports_stale_failures_and_composite_waiting_conditions() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"

[[artifact_types]]
name = "implementation"

[[protocols]]
name = "verify"
requires = ["implementation"]
trigger = { type = "on_artifact", name = "constraints" }

[[protocols]]
name = "release"
trigger = { type = "all_of", conditions = [
    { type = "on_artifact", name = "constraints" },
    { type = "any_of", conditions = [
        { type = "on_artifact", name = "implementation" },
        { type = "on_invalid", name = "constraints" }
    ] }
] }
"#,
        &[
            (
                "constraints",
                r#"{"type": "object", "required": ["title"], "properties": {"title": {"type": "string"}}}"#,
            ),
            (
                "implementation",
                r#"{"type": "object", "required": ["done"], "properties": {"done": {"type": "boolean"}}}"#,
            ),
        ],
        &["verify", "release"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::create_dir_all(workspace.join("implementation")).unwrap();
    fs::write(
        workspace.join("constraints/spec-1.json"),
        r#"{"title":"ship state"}"#,
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
        .arg("state")
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
    let protocols = value["protocols"].as_array().unwrap();
    assert_eq!(protocols.len(), 2, "{value:#}");
    assert_eq!(value["scan_warnings"], serde_json::json!([]));

    assert_eq!(protocols[0]["name"], "verify");
    assert_eq!(protocols[0]["status"], "blocked");
    assert_eq!(
        protocols[0]["precondition_failures"],
        serde_json::json!([
            {
                "artifact_type": "implementation",
                "reason": "stale"
            }
        ])
    );

    assert_eq!(protocols[1]["name"], "release");
    assert_eq!(protocols[1]["status"], "waiting");
    assert_eq!(
        protocols[1]["unsatisfied_conditions"],
        serde_json::json!([
            "on_artifact(implementation): no valid instances of artifact type 'implementation' exist",
            "on_invalid(constraints): no invalid instances of artifact type 'constraints'"
        ])
    );
}

#[test]
fn state_errors_on_uninitialized_project() {
    let dir = tempfile::tempdir().unwrap();

    let output = runa_bin()
        .arg("state")
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(!output.status.success(), "state should fail without init");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no config found"), "stderr: {stderr}");
}

#[cfg(unix)]
#[test]
fn state_keeps_skills_ready_when_only_accepted_types_are_partially_scanned() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        manifest_toml(),
        manifest_toml_schemas(),
        manifest_toml_protocols(),
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::create_dir_all(workspace.join("prior-art")).unwrap();
    fs::write(
        workspace.join("constraints/spec-1.json"),
        r#"{"title":"ship state"}"#,
    )
    .unwrap();
    let unreadable = workspace.join("prior-art/survey-1.json");
    fs::write(&unreadable, r#"{"source":"notes"}"#).unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o0)).unwrap();

    let output = runa_bin()
        .arg("state")
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

    let protocols = value["protocols"].as_array().unwrap();
    assert_eq!(protocols[0]["name"], "implement");
    assert_eq!(protocols[0]["status"], "ready");
    assert_eq!(
        protocols[0]["inputs"],
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
fn state_blocks_skills_with_partial_required_types_and_reports_scan_warnings() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        manifest_toml(),
        manifest_toml_schemas(),
        manifest_toml_protocols(),
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::create_dir_all(workspace.join("prior-art")).unwrap();
    fs::write(
        workspace.join("constraints/spec-1.json"),
        r#"{"title":"ship state"}"#,
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
        .arg("state")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    let text_output = runa_bin()
        .arg("state")
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

    let protocols = value["protocols"].as_array().unwrap();
    assert_eq!(protocols[0]["name"], "implement");
    assert_eq!(protocols[0]["status"], "blocked");
    assert_eq!(protocols[0]["trigger"], "satisfied");
    assert_eq!(
        protocols[0]["precondition_failures"],
        serde_json::json!([
            {
                "artifact_type": "constraints",
                "reason": "scan_incomplete"
            }
        ])
    );
    assert!(protocols[0].get("inputs").is_none());

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
fn state_blocks_skills_when_partial_scan_affects_requires_even_if_trigger_is_unsatisfied() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"

[[protocols]]
name = "implement"
requires = ["constraints"]
trigger = { type = "on_invalid", name = "constraints" }
"#,
        &[(
            "constraints",
            r#"{"type": "object", "required": ["title"], "properties": {"title": {"type": "string"}}}"#,
        )],
        &["implement"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::write(
        workspace.join("constraints/spec-1.json"),
        r#"{"title":"ship state"}"#,
    )
    .unwrap();
    let unreadable = workspace.join("constraints/spec-2.json");
    fs::write(&unreadable, r#"{"title":"hidden"}"#).unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o0)).unwrap();

    let output = runa_bin()
        .arg("state")
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
    let protocols = value["protocols"].as_array().unwrap();
    assert_eq!(protocols[0]["name"], "implement");
    assert_eq!(protocols[0]["status"], "blocked");
    assert_eq!(protocols[0]["trigger"], "not_satisfied");
    assert_eq!(
        protocols[0]["precondition_failures"],
        serde_json::json!([
            {
                "artifact_type": "constraints",
                "reason": "scan_incomplete"
            }
        ])
    );
    assert!(protocols[0].get("unsatisfied_conditions").is_none());
}

#[cfg(unix)]
#[test]
fn state_reports_all_partial_required_types_as_scan_incomplete_failures() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"

[[artifact_types]]
name = "implementation"

[[protocols]]
name = "verify"
requires = ["constraints", "implementation"]
trigger = { type = "on_artifact", name = "constraints" }
"#,
        &[
            (
                "constraints",
                r#"{"type": "object", "required": ["title"], "properties": {"title": {"type": "string"}}}"#,
            ),
            (
                "implementation",
                r#"{"type": "object", "required": ["done"], "properties": {"done": {"type": "boolean"}}}"#,
            ),
        ],
        &["verify"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::create_dir_all(workspace.join("implementation")).unwrap();
    fs::write(
        workspace.join("constraints/spec-1.json"),
        r#"{"title":"ship state"}"#,
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
        .arg("state")
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
    let protocols = value["protocols"].as_array().unwrap();
    assert_eq!(protocols[0]["name"], "verify");
    assert_eq!(protocols[0]["status"], "blocked");
    assert_eq!(
        protocols[0]["precondition_failures"],
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
fn state_blocks_skills_when_partial_scan_affects_trigger_only_artifact_types() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "report"

[[protocols]]
name = "repair"
trigger = { type = "on_artifact", name = "report" }
"#,
        &[(
            "report",
            r#"{"type": "object", "required": ["title"], "properties": {"title": {"type": "string"}}}"#,
        )],
        &["repair"],
    );

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
        .arg("state")
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
    let protocols = value["protocols"].as_array().unwrap();
    assert_eq!(protocols[0]["name"], "repair");
    assert_eq!(protocols[0]["status"], "blocked");
    assert_eq!(protocols[0]["trigger"], "satisfied");
    assert_eq!(
        protocols[0]["precondition_failures"],
        serde_json::json!([
            {
                "artifact_type": "report",
                "reason": "scan_incomplete"
            }
        ])
    );
}

#[test]
fn state_preserves_reason_for_empty_any_of_triggers() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

artifact_types = []

[[protocols]]
name = "impossible"
trigger = { type = "any_of", conditions = [] }
"#,
        &[],
        &["impossible"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let json_output = runa_bin()
        .arg("state")
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
    let protocols = value["protocols"].as_array().unwrap();
    assert_eq!(protocols[0]["name"], "impossible");
    assert_eq!(protocols[0]["status"], "waiting");
    assert_eq!(
        protocols[0]["unsatisfied_conditions"],
        serde_json::json!(["any_of(): any_of with no conditions"])
    );

    let text_output = runa_bin()
        .arg("state")
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
fn state_blocks_on_invalid_when_candidate_discovery_scan_trust_is_missing() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "report"

[[protocols]]
name = "repair"
trigger = { type = "on_invalid", name = "report" }
"#,
        &[(
            "report",
            r#"{"type": "object", "required": ["title"], "properties": {"title": {"type": "string"}}}"#,
        )],
        &["repair"],
    );

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
        .arg("state")
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
    let protocols = value["protocols"].as_array().unwrap();
    assert_eq!(protocols[0]["name"], "repair");
    assert_eq!(protocols[0]["status"], "blocked");
    assert_eq!(protocols[0]["trigger"], "satisfied");
    assert_eq!(
        protocols[0]["precondition_failures"],
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
fn state_blocks_any_of_when_candidate_discovery_scan_trust_is_missing() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"

[[artifact_types]]
name = "report"

[[protocols]]
name = "implement"
trigger = { type = "any_of", conditions = [
    { type = "on_invalid", name = "constraints" },
    { type = "on_artifact", name = "report" }
] }
"#,
        &[
            (
                "constraints",
                r#"{"type": "object", "required": ["title"], "properties": {"title": {"type": "string"}}}"#,
            ),
            (
                "report",
                r#"{"type": "object", "required": ["title"], "properties": {"title": {"type": "string"}}}"#,
            ),
        ],
        &["implement"],
    );

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
        .arg("state")
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
    let protocols = value["protocols"].as_array().unwrap();
    assert_eq!(protocols[0]["name"], "implement");
    assert_eq!(protocols[0]["status"], "blocked");
    assert_eq!(protocols[0]["trigger"], "satisfied");
    assert_eq!(
        protocols[0]["precondition_failures"],
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
fn state_blocks_on_artifact_when_only_unreadable_instances_exist() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "report"

[[protocols]]
name = "publish"
trigger = { type = "on_artifact", name = "report" }
"#,
        &[(
            "report",
            r#"{"type": "object", "required": ["title"], "properties": {"title": {"type": "string"}}}"#,
        )],
        &["publish"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("report")).unwrap();
    let unreadable = workspace.join("report/hidden.json");
    fs::write(&unreadable, r#"{"title":"hidden"}"#).unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o0)).unwrap();

    let output = runa_bin()
        .arg("state")
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
    let protocols = value["protocols"].as_array().unwrap();
    assert_eq!(protocols[0]["name"], "publish");
    assert_eq!(protocols[0]["status"], "blocked");
    assert_eq!(protocols[0]["trigger"], "not_satisfied");
    assert_eq!(
        protocols[0]["precondition_failures"],
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
fn state_blocks_untrustworthy_not_satisfied_on_invalid_and_on_change_triggers() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "report"

[[artifact_types]]
name = "doc"

[[protocols]]
name = "repair"
trigger = { type = "on_invalid", name = "report" }

[[protocols]]
name = "review"
trigger = { type = "on_change", name = "doc" }
"#,
        &[
            (
                "report",
                r#"{"type": "object", "required": ["title"], "properties": {"title": {"type": "string"}}}"#,
            ),
            (
                "doc",
                r#"{"type": "object", "required": ["title"], "properties": {"title": {"type": "string"}}}"#,
            ),
        ],
        &["repair", "review"],
    );

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
        .arg("state")
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
    let protocols = value["protocols"].as_array().unwrap();
    let repair = protocols
        .iter()
        .find(|protocol| protocol["name"] == "repair")
        .unwrap();
    let review = protocols
        .iter()
        .find(|protocol| protocol["name"] == "review")
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
fn state_blocks_on_change_when_output_freshness_is_untrustworthy() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "doc"

[[artifact_types]]
name = "reviewed"

[[protocols]]
name = "review"
produces = ["reviewed"]
trigger = { type = "on_change", name = "doc" }
"#,
        &[
            (
                "doc",
                r#"{"type": "object", "required": ["title"], "properties": {"title": {"type": "string"}}}"#,
            ),
            (
                "reviewed",
                r#"{"type": "object", "required": ["title"], "properties": {"title": {"type": "string"}}}"#,
            ),
        ],
        &["review"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("doc")).unwrap();
    fs::create_dir_all(workspace.join("reviewed")).unwrap();
    fs::write(workspace.join("doc/input.json"), r#"{"title":"draft"}"#).unwrap();
    let unreadable_output = workspace.join("reviewed/hidden.json");
    fs::write(&unreadable_output, r#"{"title":"done"}"#).unwrap();
    fs::set_permissions(&unreadable_output, fs::Permissions::from_mode(0o0)).unwrap();

    let output = runa_bin()
        .arg("state")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    fs::set_permissions(&unreadable_output, fs::Permissions::from_mode(0o644)).unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let review = value["protocols"]
        .as_array()
        .unwrap()
        .iter()
        .find(|protocol| protocol["name"] == "review")
        .unwrap();

    assert_eq!(review["status"], "ready");
    assert_eq!(review["trigger"], "satisfied");
    assert!(review.get("precondition_failures").is_none());
}

#[cfg(unix)]
#[test]
fn state_scoped_partial_outputs_reopen_each_delegated_work_unit() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "doc"

[[artifact_types]]
name = "reviewed"

[[protocols]]
name = "review"
produces = ["reviewed"]
scoped = true
trigger = { type = "on_change", name = "doc" }
"#,
        &[
            (
                "doc",
                r#"{"type": "object", "required": ["title", "work_unit"], "properties": {"title": {"type": "string"}, "work_unit": {"type": "string"}}}"#,
            ),
            (
                "reviewed",
                r#"{"type": "object", "required": ["title", "work_unit"], "properties": {"title": {"type": "string"}, "work_unit": {"type": "string"}}}"#,
            ),
        ],
        &["review"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("doc")).unwrap();
    fs::create_dir_all(workspace.join("reviewed")).unwrap();
    fs::write(
        workspace.join("doc/a.json"),
        r#"{"title":"draft-a","work_unit":"wu-a"}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("doc/b.json"),
        r#"{"title":"draft-b","work_unit":"wu-b"}"#,
    )
    .unwrap();

    let first_scan = runa_bin()
        .arg("scan")
        .current_dir(&project_dir)
        .output()
        .unwrap();
    assert!(
        first_scan.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first_scan.stderr)
    );

    fs::write(
        workspace.join("reviewed/a.json"),
        r#"{"title":"done-a","work_unit":"wu-a"}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("reviewed/b.json"),
        r#"{"title":"done-b","work_unit":"wu-b"}"#,
    )
    .unwrap();

    let second_scan = runa_bin()
        .arg("scan")
        .current_dir(&project_dir)
        .output()
        .unwrap();
    assert!(
        second_scan.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&second_scan.stderr)
    );

    let unreadable_output = workspace.join("reviewed/hidden.json");
    fs::write(
        &unreadable_output,
        r#"{"title":"hidden","work_unit":"wu-a"}"#,
    )
    .unwrap();
    fs::set_permissions(&unreadable_output, fs::Permissions::from_mode(0o0)).unwrap();

    let wu_a_output = runa_bin()
        .arg("state")
        .arg("--json")
        .arg("--work-unit")
        .arg("wu-a")
        .current_dir(&project_dir)
        .output()
        .unwrap();
    let wu_b_output = runa_bin()
        .arg("state")
        .arg("--json")
        .arg("--work-unit")
        .arg("wu-b")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    fs::set_permissions(&unreadable_output, fs::Permissions::from_mode(0o644)).unwrap();

    for output in [&wu_a_output, &wu_b_output] {
        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let wu_a_value: serde_json::Value = serde_json::from_slice(&wu_a_output.stdout).unwrap();
    let wu_a_reviews = wu_a_value["protocols"].as_array().unwrap();
    assert_eq!(wu_a_reviews.len(), 1, "{wu_a_value:#}");
    assert_eq!(wu_a_reviews[0]["name"], "review");
    assert_eq!(wu_a_reviews[0]["work_unit"], "wu-a");
    assert_eq!(wu_a_reviews[0]["status"], "ready");

    let wu_b_value: serde_json::Value = serde_json::from_slice(&wu_b_output.stdout).unwrap();
    let wu_b_reviews = wu_b_value["protocols"].as_array().unwrap();
    assert_eq!(wu_b_reviews.len(), 1, "{wu_b_value:#}");
    assert_eq!(wu_b_reviews[0]["name"], "review");
    assert_eq!(wu_b_reviews[0]["work_unit"], "wu-b");
    assert_eq!(wu_b_reviews[0]["status"], "ready");
}

#[test]
fn state_reports_scoped_on_change_readiness_when_freshness_is_mixed() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "doc"

[[artifact_types]]
name = "reviewed"

[[protocols]]
name = "review"
produces = ["reviewed"]
scoped = true
trigger = { type = "on_change", name = "doc" }
"#,
        &[
            (
                "doc",
                r#"{"type": "object", "required": ["title", "work_unit"], "properties": {"title": {"type": "string"}, "work_unit": {"type": "string"}}}"#,
            ),
            (
                "reviewed",
                r#"{"type": "object", "required": ["title", "work_unit"], "properties": {"title": {"type": "string"}, "work_unit": {"type": "string"}}}"#,
            ),
        ],
        &["review"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("doc")).unwrap();
    fs::create_dir_all(workspace.join("reviewed")).unwrap();

    fs::write(
        workspace.join("reviewed/a.json"),
        r#"{"title":"done-a","work_unit":"wu-a"}"#,
    )
    .unwrap();
    let first_scan = runa_bin()
        .arg("scan")
        .current_dir(&project_dir)
        .output()
        .unwrap();
    assert!(
        first_scan.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first_scan.stderr)
    );
    fs::write(
        workspace.join("doc/a.json"),
        r#"{"title":"draft-a","work_unit":"wu-a"}"#,
    )
    .unwrap();
    let second_scan = runa_bin()
        .arg("scan")
        .current_dir(&project_dir)
        .output()
        .unwrap();
    assert!(
        second_scan.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&second_scan.stderr)
    );
    fs::write(
        workspace.join("doc/b.json"),
        r#"{"title":"draft-b","work_unit":"wu-b"}"#,
    )
    .unwrap();
    let third_scan = runa_bin()
        .arg("scan")
        .current_dir(&project_dir)
        .output()
        .unwrap();
    assert!(
        third_scan.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&third_scan.stderr)
    );
    fs::write(
        workspace.join("reviewed/b.json"),
        r#"{"title":"done-b","work_unit":"wu-b"}"#,
    )
    .unwrap();

    let wu_a_output = runa_bin()
        .arg("state")
        .arg("--json")
        .arg("--work-unit")
        .arg("wu-a")
        .current_dir(&project_dir)
        .output()
        .unwrap();
    let wu_b_output = runa_bin()
        .arg("state")
        .arg("--json")
        .arg("--work-unit")
        .arg("wu-b")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    for output in [&wu_a_output, &wu_b_output] {
        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let wu_a_value: serde_json::Value = serde_json::from_slice(&wu_a_output.stdout).unwrap();
    let wu_a_reviews = wu_a_value["protocols"].as_array().unwrap();
    assert_eq!(wu_a_reviews.len(), 1, "{wu_a_value:#}");
    assert_eq!(wu_a_reviews[0]["work_unit"], "wu-a");
    assert_eq!(wu_a_reviews[0]["status"], "ready");
    assert_eq!(wu_a_reviews[0]["trigger"], "satisfied");

    let wu_b_value: serde_json::Value = serde_json::from_slice(&wu_b_output.stdout).unwrap();
    let wu_b_reviews = wu_b_value["protocols"].as_array().unwrap();
    assert_eq!(wu_b_reviews.len(), 1, "{wu_b_value:#}");
    assert_eq!(wu_b_reviews[0]["work_unit"], "wu-b");
    assert_eq!(wu_b_reviews[0]["status"], "waiting");
    assert_eq!(wu_b_reviews[0]["trigger"], "not_satisfied");
}

#[cfg(unix)]
#[test]
fn state_does_not_block_on_change_for_partial_optional_outputs() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "doc"

[[artifact_types]]
name = "reviewed"

[[artifact_types]]
name = "review-notes"

[[protocols]]
name = "review"
produces = ["reviewed"]
may_produce = ["review-notes"]
trigger = { type = "on_change", name = "doc" }
"#,
        &[
            (
                "doc",
                r#"{"type": "object", "required": ["title"], "properties": {"title": {"type": "string"}}}"#,
            ),
            (
                "reviewed",
                r#"{"type": "object", "required": ["title"], "properties": {"title": {"type": "string"}}}"#,
            ),
            (
                "review-notes",
                r#"{"type": "object", "required": ["title"], "properties": {"title": {"type": "string"}}}"#,
            ),
        ],
        &["review"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("doc")).unwrap();
    fs::create_dir_all(workspace.join("reviewed")).unwrap();
    fs::create_dir_all(workspace.join("review-notes")).unwrap();
    fs::write(workspace.join("doc/input.json"), r#"{"title":"draft"}"#).unwrap();
    fs::write(workspace.join("reviewed/input.json"), r#"{"title":"done"}"#).unwrap();
    let unreadable_optional = workspace.join("review-notes/hidden.json");
    fs::write(&unreadable_optional, r#"{"title":"note"}"#).unwrap();
    fs::set_permissions(&unreadable_optional, fs::Permissions::from_mode(0o0)).unwrap();

    let output = runa_bin()
        .arg("state")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    fs::set_permissions(&unreadable_optional, fs::Permissions::from_mode(0o644)).unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let review = value["protocols"]
        .as_array()
        .unwrap()
        .iter()
        .find(|protocol| protocol["name"] == "review")
        .unwrap();

    assert_eq!(review["status"], "waiting");
    assert_eq!(review["trigger"], "not_satisfied");
    assert_eq!(
        review["unsatisfied_conditions"],
        serde_json::json!([
            "on_change(doc): artifact type 'doc' has not changed since protocol outputs were last updated"
        ])
    );
}

#[cfg(unix)]
#[test]
fn state_scoped_reruns_reopen_when_output_scan_gaps_exist() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "doc"

[[artifact_types]]
name = "reviewed"

[[protocols]]
name = "review"
produces = ["reviewed"]
scoped = true
trigger = { type = "on_change", name = "doc" }
"#,
        &[
            (
                "doc",
                r#"{"type": "object", "required": ["title", "work_unit"], "properties": {"title": {"type": "string"}, "work_unit": {"type": "string"}}}"#,
            ),
            (
                "reviewed",
                r#"{"type": "object", "required": ["title", "work_unit"], "properties": {"title": {"type": "string"}, "work_unit": {"type": "string"}}}"#,
            ),
        ],
        &["review"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("doc")).unwrap();
    fs::create_dir_all(workspace.join("reviewed")).unwrap();
    fs::write(
        workspace.join("doc/a.json"),
        r#"{"title":"draft-a","work_unit":"wu-a"}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("doc/b.json"),
        r#"{"title":"draft-b","work_unit":"wu-b"}"#,
    )
    .unwrap();

    let first_scan = runa_bin()
        .arg("scan")
        .current_dir(&project_dir)
        .output()
        .unwrap();
    assert!(
        first_scan.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first_scan.stderr)
    );

    fs::write(
        workspace.join("reviewed/a.json"),
        r#"{"title":"done-a","work_unit":"wu-a"}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("reviewed/b.json"),
        r#"{"title":"done-b","work_unit":"wu-b"}"#,
    )
    .unwrap();

    let second_scan = runa_bin()
        .arg("scan")
        .current_dir(&project_dir)
        .output()
        .unwrap();
    assert!(
        second_scan.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&second_scan.stderr)
    );

    let unreadable_output = workspace.join("reviewed/a.json");
    fs::set_permissions(&unreadable_output, fs::Permissions::from_mode(0o0)).unwrap();

    let wu_a_output = runa_bin()
        .arg("state")
        .arg("--json")
        .arg("--work-unit")
        .arg("wu-a")
        .current_dir(&project_dir)
        .output()
        .unwrap();
    let wu_b_output = runa_bin()
        .arg("state")
        .arg("--json")
        .arg("--work-unit")
        .arg("wu-b")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    fs::set_permissions(&unreadable_output, fs::Permissions::from_mode(0o644)).unwrap();

    for output in [&wu_a_output, &wu_b_output] {
        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let wu_a_value: serde_json::Value = serde_json::from_slice(&wu_a_output.stdout).unwrap();
    let wu_a_reviews = wu_a_value["protocols"].as_array().unwrap();
    assert_eq!(wu_a_reviews.len(), 1, "{wu_a_value:#}");
    assert_eq!(wu_a_reviews[0]["work_unit"], "wu-a");
    assert_eq!(wu_a_reviews[0]["status"], "ready");

    let wu_b_value: serde_json::Value = serde_json::from_slice(&wu_b_output.stdout).unwrap();
    let wu_b_reviews = wu_b_value["protocols"].as_array().unwrap();
    assert_eq!(wu_b_reviews.len(), 1, "{wu_b_value:#}");
    assert_eq!(wu_b_reviews[0]["work_unit"], "wu-b");
    assert_eq!(wu_b_reviews[0]["status"], "ready");
}

#[cfg(unix)]
#[test]
fn state_scoped_outputs_ignore_unverifiable_stored_work_unit_labels() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "doc"

[[artifact_types]]
name = "reviewed"

[[protocols]]
name = "review"
produces = ["reviewed"]
scoped = true
trigger = { type = "on_change", name = "doc" }
"#,
        &[
            (
                "doc",
                r#"{"type": "object", "required": ["title", "work_unit"], "properties": {"title": {"type": "string"}, "work_unit": {"type": "string"}}}"#,
            ),
            (
                "reviewed",
                r#"{"type": "object", "required": ["title", "work_unit"], "properties": {"title": {"type": "string"}, "work_unit": {"type": "string"}}}"#,
            ),
        ],
        &["review"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("doc")).unwrap();
    fs::create_dir_all(workspace.join("reviewed")).unwrap();
    fs::write(
        workspace.join("doc/a.json"),
        r#"{"title":"draft-a","work_unit":"wu-a"}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("doc/b.json"),
        r#"{"title":"draft-b","work_unit":"wu-b"}"#,
    )
    .unwrap();

    let first_scan = runa_bin()
        .arg("scan")
        .current_dir(&project_dir)
        .output()
        .unwrap();
    assert!(
        first_scan.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first_scan.stderr)
    );

    fs::write(
        workspace.join("reviewed/a.json"),
        r#"{"title":"done-a","work_unit":"wu-a"}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("reviewed/b.json"),
        r#"{"title":"done-b","work_unit":"wu-b"}"#,
    )
    .unwrap();

    let second_scan = runa_bin()
        .arg("scan")
        .current_dir(&project_dir)
        .output()
        .unwrap();
    assert!(
        second_scan.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&second_scan.stderr)
    );

    let unreadable_output = workspace.join("reviewed/a.json");
    fs::write(
        &unreadable_output,
        r#"{"title":"done-a","work_unit":"wu-b"}"#,
    )
    .unwrap();
    fs::set_permissions(&unreadable_output, fs::Permissions::from_mode(0o0)).unwrap();

    let wu_a_output = runa_bin()
        .arg("state")
        .arg("--json")
        .arg("--work-unit")
        .arg("wu-a")
        .current_dir(&project_dir)
        .output()
        .unwrap();
    let wu_b_output = runa_bin()
        .arg("state")
        .arg("--json")
        .arg("--work-unit")
        .arg("wu-b")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    fs::set_permissions(&unreadable_output, fs::Permissions::from_mode(0o644)).unwrap();

    for output in [&wu_a_output, &wu_b_output] {
        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let wu_a_value: serde_json::Value = serde_json::from_slice(&wu_a_output.stdout).unwrap();
    let wu_a_reviews = wu_a_value["protocols"].as_array().unwrap();
    assert_eq!(wu_a_reviews.len(), 1, "{wu_a_value:#}");
    assert_eq!(wu_a_reviews[0]["work_unit"], "wu-a");
    assert_eq!(wu_a_reviews[0]["status"], "ready");

    let wu_b_value: serde_json::Value = serde_json::from_slice(&wu_b_output.stdout).unwrap();
    let wu_b_reviews = wu_b_value["protocols"].as_array().unwrap();
    assert_eq!(wu_b_reviews.len(), 1, "{wu_b_value:#}");
    assert_eq!(wu_b_reviews[0]["work_unit"], "wu-b");
    assert_eq!(wu_b_reviews[0]["status"], "ready");
}

#[cfg(unix)]
#[test]
fn state_preserves_invalid_preconditions_alongside_scan_incomplete() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"

[[protocols]]
name = "implement"
requires = ["constraints"]
trigger = { type = "on_artifact", name = "constraints" }
"#,
        &[(
            "constraints",
            r#"{"type": "object", "required": ["title"], "properties": {"title": {"type": "string"}}}"#,
        )],
        &["implement"],
    );

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
        .arg("state")
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
    let protocols = value["protocols"].as_array().unwrap();
    assert_eq!(protocols[0]["name"], "implement");
    assert_eq!(protocols[0]["status"], "blocked");
    assert_eq!(protocols[0]["trigger"], "not_satisfied");
    assert_eq!(
        protocols[0]["precondition_failures"],
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
fn state_blocks_on_artifact_when_partial_scan_could_hide_a_valid_instance() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "report"

[[protocols]]
name = "publish"
trigger = { type = "on_artifact", name = "report" }
"#,
        &[(
            "report",
            r#"{"type": "object", "required": ["title"], "properties": {"title": {"type": "string"}}}"#,
        )],
        &["publish"],
    );

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
        .arg("state")
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
    let protocols = value["protocols"].as_array().unwrap();
    assert_eq!(protocols[0]["name"], "publish");
    assert_eq!(protocols[0]["status"], "blocked");
    assert_eq!(protocols[0]["trigger"], "not_satisfied");
    assert_eq!(
        protocols[0]["precondition_failures"],
        serde_json::json!([
            {
                "artifact_type": "report",
                "reason": "scan_incomplete"
            }
        ])
    );
    assert!(protocols[0].get("unsatisfied_conditions").is_none());
}

#[test]
fn state_marks_trigger_and_requires_ready_when_a_valid_sibling_exists() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "request"

[[protocols]]
name = "publish"
requires = ["request"]
trigger = { type = "on_artifact", name = "request" }
"#,
        &[(
            "request",
            r#"{"type": "object", "required": ["title"], "properties": {"title": {"type": "string"}}}"#,
        )],
        &["publish"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("request")).unwrap();
    fs::write(workspace.join("request/good.json"), r#"{"title":"ok"}"#).unwrap();
    fs::write(workspace.join("request/bad.json"), r#"{"bad":true}"#).unwrap();

    let output = runa_bin()
        .arg("state")
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
    let protocols = value["protocols"].as_array().unwrap();
    assert_eq!(protocols.len(), 1, "{value:#}");
    assert_eq!(protocols[0]["name"], "publish");
    assert_eq!(protocols[0]["status"], "ready");
    assert_eq!(protocols[0]["trigger"], "satisfied");
    assert_eq!(
        protocols[0]["inputs"],
        serde_json::json!([
            {
                "artifact_type": "request",
                "instance_id": "good",
                "path": ".runa/workspace/request/good.json",
                "relationship": "requires"
            }
        ])
    );
    assert!(protocols[0].get("precondition_failures").is_none());
    assert!(protocols[0].get("unsatisfied_conditions").is_none());
}
