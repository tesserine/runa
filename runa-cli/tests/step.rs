mod common;

use std::fs;
use std::path::Path;
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

fn methodology_schemas() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "constraints",
            r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
        ),
        (
            "prior-art",
            r#"{"type":"object","required":["source"],"properties":{"source":{"type":"string"}}}"#,
        ),
        (
            "implementation",
            r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#,
        ),
    ]
}

fn methodology_protocols() -> Vec<&'static str> {
    vec!["implement", "verify", "ground"]
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

fn append_agent_command_config(project_dir: &Path, command: &[&Path]) {
    let config_path = project_dir.join(".runa/config.toml");
    let existing = fs::read_to_string(&config_path).unwrap();
    let command_entries = command
        .iter()
        .map(|entry| format!("  {:?},", entry.display().to_string()))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(
        config_path,
        format!("{existing}\n[agent]\ncommand = [\n{command_entries}\n]\n"),
    )
    .unwrap();
}

#[cfg(unix)]
fn write_capture_agent(dir: &Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let script_path = dir.join("capture-agent.sh");
    fs::write(&script_path, "#!/bin/sh\ncat > \"$1\"\n").unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    script_path
}

#[cfg(unix)]
fn write_failing_agent(dir: &Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let script_path = dir.join("failing-agent.sh");
    fs::write(
        &script_path,
        "#!/bin/sh\ncount_file=\"$1\"\npayload_dir=\"$2\"\ncount=0\nif [ -f \"$count_file\" ]; then count=$(cat \"$count_file\"); fi\ncount=$((count + 1))\nprintf '%s' \"$count\" > \"$count_file\"\ncat > \"$payload_dir/$count.json\"\nexit 17\n",
    )
    .unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    script_path
}

#[test]
fn step_dry_run_json_reports_ready_execution_plan_and_full_skill_status() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        manifest_toml(),
        &methodology_schemas(),
        &methodology_protocols(),
    );

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
    assert_eq!(value["version"], 2);
    assert_eq!(value["methodology"], "groundwork");
    assert_eq!(value["scan_warnings"], serde_json::json!([]));
    assert!(value.get("cycle").is_none(), "{value:#}");

    let execution_plan = value["execution_plan"].as_array().unwrap();
    assert_eq!(execution_plan.len(), 1, "{value:#}");
    assert_eq!(execution_plan[0]["protocol"], "implement");
    assert_eq!(execution_plan[0]["trigger"], "on_artifact(constraints)");
    assert_eq!(execution_plan[0]["context"]["protocol"], "implement");
    assert!(
        execution_plan[0]["context"]["work_unit"].is_null(),
        "{value:#}"
    );
    assert_eq!(
        execution_plan[0]["context"]["instructions"],
        "# implement\n"
    );
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

    let protocols = value["protocols"].as_array().unwrap();
    assert_eq!(protocols.len(), 3, "{value:#}");
    assert_eq!(protocols[0]["name"], "implement");
    assert_eq!(protocols[0]["status"], "ready");
    assert_eq!(protocols[1]["name"], "verify");
    assert_eq!(protocols[1]["status"], "blocked");
    assert_eq!(protocols[2]["name"], "ground");
    assert_eq!(protocols[2]["status"], "waiting");
    assert_eq!(
        protocols[2]["unsatisfied_conditions"],
        serde_json::json!([
            "on_invalid(implementation): no invalid instances of artifact type 'implementation'"
        ])
    );
}

#[test]
fn step_dry_run_text_reports_why_when_no_skills_are_ready() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        manifest_toml(),
        &methodology_schemas(),
        &methodology_protocols(),
    );

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
    assert!(stdout.contains("No READY protocols."), "stdout: {stdout}");
    assert!(stdout.contains("WAITING:"), "stdout: {stdout}");
    assert!(
        stdout.contains(
            "on_artifact(constraints): no valid instances of artifact type 'constraints' exist"
        ),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains(
            "on_invalid(implementation): no invalid instances of artifact type 'implementation'"
        ),
        "stdout: {stdout}"
    );
}

#[test]
fn step_dry_run_text_shows_preloaded_protocol_instructions_for_ready_protocols() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        manifest_toml(),
        &methodology_schemas(),
        &methodology_protocols(),
    );

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
        stdout.contains("\"protocol\": \"implement\""),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("\"instructions\": \"# implement\\n\""),
        "stdout: {stdout}"
    );
}

#[test]
fn step_without_dry_run_fails_when_agent_command_is_not_configured() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        manifest_toml(),
        &methodology_schemas(),
        &methodology_protocols(),
    );

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
    assert!(stderr.contains("ERROR"), "stderr: {stderr}");
    assert!(stderr.contains("command"), "stderr: {stderr}");
    assert!(stderr.contains("step"), "stderr: {stderr}");
    assert!(
        stderr.contains("no agent command configured"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("[agent]"), "stderr: {stderr}");
    assert!(stderr.contains("config.toml"), "stderr: {stderr}");
}

#[cfg(unix)]
#[test]
fn step_without_dry_run_invokes_configured_agent_with_execution_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        manifest_toml(),
        &methodology_schemas(),
        &methodology_protocols(),
    );

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

    let payload_path = dir.path().join("captured-payload.json");
    let agent_path = write_capture_agent(dir.path());
    append_agent_command_config(
        &project_dir,
        &[agent_path.as_path(), payload_path.as_path()],
    );

    let output = runa_bin()
        .arg("step")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let captured = fs::read_to_string(&payload_path).unwrap();
    assert!(captured.contains("# Protocol: implement"), "{captured}");
    assert!(captured.contains("## Protocol instructions"), "{captured}");
    assert!(captured.contains("# implement"), "{captured}");
    assert!(captured.contains("## What you've been given"), "{captured}");
    assert!(captured.contains("**Title:** ship step"), "{captured}");
    assert!(captured.contains("## Additional context"), "{captured}");
    assert!(captured.contains("**Source:** notes"), "{captured}");
    assert!(
        captured.contains("## What you need to deliver"),
        "{captured}"
    );
    assert!(
        captured.contains("You must produce: implementation"),
        "{captured}"
    );
}

#[test]
fn step_without_dry_run_rejects_json_output() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        manifest_toml(),
        &methodology_schemas(),
        &methodology_protocols(),
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let output = runa_bin()
        .arg("step")
        .arg("--json")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(!output.status.success(), "step should reject --json");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--json is only supported with --dry-run"));
}

#[test]
fn step_without_dry_run_reports_project_load_failure_before_agent_config_failure() {
    let dir = tempfile::tempdir().unwrap();
    let external_config = dir.path().join("external-config.toml");
    fs::write(
        &external_config,
        r#"
methodology_path = "/tmp/methodology.toml"
"#,
    )
    .unwrap();

    let project_dir = dir.path().join("not-a-project");
    fs::create_dir(&project_dir).unwrap();

    let output = runa_bin()
        .arg("--config")
        .arg(&external_config)
        .arg("step")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "step should fail outside an initialized project"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not a runa project"), "stderr: {stderr}");
    assert!(
        !stderr.contains("no agent command configured"),
        "stderr: {stderr}"
    );
}

#[cfg(unix)]
#[test]
fn step_without_dry_run_stops_after_first_non_zero_agent_exit() {
    let dir = tempfile::tempdir().unwrap();
    let wu_schema = r#"{"type":"object","required":["title","work_unit"],"properties":{"title":{"type":"string"},"work_unit":{"type":"string"}}}"#;
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
requires = ["doc"]
produces = ["reviewed"]
trigger = { type = "on_artifact", name = "doc" }
"#,
        &[("doc", wu_schema), ("reviewed", wu_schema)],
        &["review"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("doc")).unwrap();
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

    let count_file = dir.path().join("count.txt");
    let payload_dir = dir.path().join("payloads");
    fs::create_dir_all(&payload_dir).unwrap();
    let agent_path = write_failing_agent(dir.path());
    append_agent_command_config(
        &project_dir,
        &[
            agent_path.as_path(),
            count_file.as_path(),
            payload_dir.as_path(),
        ],
    );

    let output = runa_bin()
        .arg("step")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "step should fail on non-zero agent exit"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("exited with status 17"), "stderr: {stderr}");
    assert!(stderr.contains("protocol 'review'"), "stderr: {stderr}");
    assert!(stderr.contains("work_unit=wu-a"), "stderr: {stderr}");
    assert_eq!(fs::read_to_string(&count_file).unwrap(), "1");
    let captured = fs::read_to_string(payload_dir.join("1.json")).unwrap();
    assert!(
        captured.contains("# Protocol: review (work_unit=wu-a)"),
        "{captured}"
    );
    assert!(payload_dir.join("1.json").is_file());
    assert!(!payload_dir.join("2.json").exists());
}

#[test]
fn step_dry_run_reports_blocked_reasons_when_no_skills_are_ready() {
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
"#,
        &[
            (
                "constraints",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            (
                "implementation",
                r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#,
            ),
        ],
        &["verify"],
    );

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
    assert!(stdout.contains("No READY protocols."), "stdout: {stdout}");
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
    let manifest_path = common::write_methodology(
        dir.path(),
        manifest_toml(),
        &methodology_schemas(),
        &methodology_protocols(),
    );

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

    let protocols = value["protocols"].as_array().unwrap();
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

#[test]
fn step_dry_run_json_reports_cycle_and_omits_execution_plan() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "a"

[[artifact_types]]
name = "b"

[[protocols]]
name = "first"
requires = ["b"]
produces = ["a"]
trigger = { type = "on_change", name = "b" }

[[protocols]]
name = "second"
requires = ["a"]
produces = ["b"]
trigger = { type = "on_change", name = "b" }
"#,
        &[
            (
                "a",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            (
                "b",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
        ],
        &["first", "second"],
    );

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
    let protocols = value["protocols"].as_array().unwrap();
    assert_eq!(protocols.len(), 2, "{value:#}");
    assert_eq!(protocols[0]["name"], "first");
    assert_eq!(protocols[1]["name"], "second");
}

#[test]
fn step_dry_run_text_reports_cycle_and_no_execution_plan() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "a"

[[artifact_types]]
name = "b"

[[protocols]]
name = "first"
requires = ["b"]
produces = ["a"]
trigger = { type = "on_change", name = "b" }

[[protocols]]
name = "second"
requires = ["a"]
produces = ["b"]
trigger = { type = "on_change", name = "b" }
"#,
        &[
            (
                "a",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            (
                "b",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
        ],
        &["first", "second"],
    );

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
    let title_schema =
        r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#;
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "seed"

[[artifact_types]]
name = "a"

[[artifact_types]]
name = "b"

[[artifact_types]]
name = "result"

[[protocols]]
name = "independent"
requires = ["seed"]
produces = ["result"]
trigger = { type = "on_artifact", name = "seed" }

[[protocols]]
name = "first"
requires = ["b"]
produces = ["a"]
trigger = { type = "on_change", name = "b" }

[[protocols]]
name = "second"
requires = ["a"]
produces = ["b"]
trigger = { type = "on_change", name = "b" }
"#,
        &[
            ("seed", title_schema),
            ("a", title_schema),
            ("b", title_schema),
            (
                "result",
                r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#,
            ),
        ],
        &["independent", "first", "second"],
    );

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
    assert_eq!(execution_plan[0]["protocol"], "independent");

    let protocols = value["protocols"].as_array().unwrap();
    assert_eq!(protocols.len(), 3, "{value:#}");
    assert_eq!(protocols[0]["name"], "independent");
}

#[test]
fn step_dry_run_keeps_ready_skills_downstream_of_cycle_when_inputs_exist() {
    let dir = tempfile::tempdir().unwrap();
    let title_schema =
        r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#;
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "a"

[[artifact_types]]
name = "b"

[[artifact_types]]
name = "result"

[[protocols]]
name = "first"
requires = ["b"]
produces = ["a"]
trigger = { type = "on_change", name = "b" }

[[protocols]]
name = "second"
requires = ["a"]
produces = ["b"]
trigger = { type = "on_change", name = "b" }

[[protocols]]
name = "publish"
requires = ["a"]
produces = ["result"]
trigger = { type = "on_artifact", name = "a" }
"#,
        &[
            ("a", title_schema),
            ("b", title_schema),
            (
                "result",
                r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#,
            ),
        ],
        &["first", "second", "publish"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("a")).unwrap();
    fs::write(
        workspace.join("a/input.json"),
        r#"{"title":"already here"}"#,
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
    assert_eq!(
        value["cycle"],
        serde_json::json!(["first", "second"]),
        "{value:#}"
    );

    let execution_plan = value["execution_plan"].as_array().unwrap();
    assert_eq!(execution_plan.len(), 1, "{value:#}");
    assert_eq!(execution_plan[0]["protocol"], "publish");

    let protocols = value["protocols"].as_array().unwrap();
    assert_eq!(protocols[0]["name"], "publish");
    assert_eq!(protocols[0]["status"], "ready");
}

#[test]
fn step_dry_run_preserves_dependency_order_for_ready_skills_with_unrelated_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let title_schema =
        r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#;
    let manifest_path = common::write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "root"

[[artifact_types]]
name = "seed"

[[artifact_types]]
name = "result"

[[artifact_types]]
name = "a"

[[artifact_types]]
name = "b"

[[protocols]]
name = "independent"
requires = ["root"]
produces = ["seed"]
trigger = { type = "on_artifact", name = "root" }

[[protocols]]
name = "publish"
requires = ["seed"]
produces = ["result"]
trigger = { type = "on_artifact", name = "seed" }

[[protocols]]
name = "first"
requires = ["b"]
produces = ["a"]
trigger = { type = "on_change", name = "b" }

[[protocols]]
name = "second"
requires = ["a"]
produces = ["b"]
trigger = { type = "on_change", name = "b" }
"#,
        &[
            ("root", title_schema),
            ("seed", title_schema),
            (
                "result",
                r#"{"type":"object","required":["done"],"properties":{"done":{"type":"boolean"}}}"#,
            ),
            ("a", title_schema),
            ("b", title_schema),
        ],
        &["independent", "publish", "first", "second"],
    );

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
    assert_eq!(execution_plan.len(), 1, "{value:#}");
    assert_eq!(execution_plan[0]["protocol"], "publish");
}

#[test]
fn step_dry_run_reports_per_work_unit_on_change_readiness_when_freshness_is_mixed() {
    let dir = tempfile::tempdir().unwrap();
    let wu_schema = r#"{"type":"object","required":["title","work_unit"],"properties":{"title":{"type":"string"},"work_unit":{"type":"string"}}}"#;
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
        &[("doc", wu_schema), ("reviewed", wu_schema)],
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
    assert_eq!(execution_plan.len(), 1, "{value:#}");
    assert_eq!(execution_plan[0]["protocol"], "review");
    assert_eq!(execution_plan[0]["work_unit"], "wu-a");

    let reviews: Vec<_> = value["protocols"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|protocol| protocol["name"] == "review")
        .collect();
    assert_eq!(reviews.len(), 2, "{value:#}");
    let ready = reviews
        .iter()
        .find(|protocol| protocol["status"] == "ready")
        .unwrap();
    let waiting = reviews
        .iter()
        .find(|protocol| protocol["status"] == "waiting")
        .unwrap();
    assert_eq!(ready["work_unit"], "wu-a");
    assert_eq!(waiting["work_unit"], "wu-b");
}
