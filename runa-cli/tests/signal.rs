use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn runa_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_runa"))
}

fn manifest_toml() -> &'static str {
    r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"
schema = { type = "object", required = ["title"], properties = { title = { type = "string" } } }

[[skills]]
name = "ground"
trigger = { type = "on_signal", name = "begin" }

[[skills]]
name = "implement"
requires = ["constraints"]
trigger = { type = "on_artifact", name = "constraints" }
"#
}

fn run_command(project_dir: &Path, args: &[&str]) -> Output {
    runa_bin()
        .args(args)
        .current_dir(project_dir)
        .output()
        .unwrap()
}

fn init_project(project_dir: &Path, manifest_path: &Path) {
    let output = run_command(
        project_dir,
        &["init", "--methodology", manifest_path.to_str().unwrap()],
    );
    assert!(
        output.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_project_with_args(project_dir: &Path, args: &[&str]) {
    let output = run_command(project_dir, args);
    assert!(
        output.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn signal_begin_persists_state_across_invocations() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(&manifest_path, manifest_toml()).unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let begin = run_command(&project_dir, &["signal", "begin", "begin"]);
    assert!(
        begin.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&begin.stderr)
    );

    let list = run_command(&project_dir, &["signal", "list"]);
    assert!(
        list.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&list.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&list.stdout).trim(), "begin");

    let status = run_command(&project_dir, &["status", "--json"]);
    assert!(
        status.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(value["skills"][0]["name"], "ground");
    assert_eq!(value["skills"][0]["status"], "ready");
}

#[test]
fn signal_begin_is_idempotent_and_deduplicates_storage() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(&manifest_path, manifest_toml()).unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    for _ in 0..2 {
        let output = run_command(&project_dir, &["signal", "begin", "begin"]);
        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stored: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(project_dir.join(".runa/signals.json")).unwrap())
            .unwrap();
    assert_eq!(stored, serde_json::json!({ "active": ["begin"] }));
}

#[test]
fn signal_clear_removes_signal_and_step_waits_again() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(&manifest_path, manifest_toml()).unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let begin = run_command(&project_dir, &["signal", "begin", "begin"]);
    assert!(begin.status.success());

    let step_ready = run_command(&project_dir, &["step", "--dry-run", "--json"]);
    assert!(
        step_ready.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&step_ready.stderr)
    );
    let ready_value: serde_json::Value = serde_json::from_slice(&step_ready.stdout).unwrap();
    assert_eq!(ready_value["execution_plan"][0]["skill"], "ground");

    let clear = run_command(&project_dir, &["signal", "clear", "begin"]);
    assert!(
        clear.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&clear.stderr)
    );

    let clear_again = run_command(&project_dir, &["signal", "clear", "begin"]);
    assert!(
        clear_again.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&clear_again.stderr)
    );

    let step_waiting = run_command(&project_dir, &["step", "--dry-run", "--json"]);
    assert!(
        step_waiting.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&step_waiting.stderr)
    );
    let waiting_value: serde_json::Value = serde_json::from_slice(&step_waiting.stdout).unwrap();
    assert_eq!(waiting_value["execution_plan"], serde_json::json!([]));
    assert_eq!(waiting_value["skills"][1]["name"], "ground");
    assert_eq!(waiting_value["skills"][1]["status"], "waiting");

    let status_waiting = run_command(&project_dir, &["status", "--json"]);
    assert!(
        status_waiting.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&status_waiting.stderr)
    );
    let status_value: serde_json::Value = serde_json::from_slice(&status_waiting.stdout).unwrap();
    let waiting_entry = status_value["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "ground")
        .unwrap();
    assert_eq!(waiting_entry["status"], "waiting");
}

#[test]
fn signal_list_reports_empty_when_no_signals_are_active() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(&manifest_path, manifest_toml()).unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let output = run_command(&project_dir, &["signal", "list"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "No active signals."
    );
}

#[test]
fn signal_list_sorts_active_signals() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(&manifest_path, manifest_toml()).unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);
    fs::write(
        project_dir.join(".runa/signals.json"),
        r#"{ "active": ["deploy", "begin", "approve"] }"#,
    )
    .unwrap();

    let output = run_command(&project_dir, &["signal", "list"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "approve\nbegin\ndeploy\n"
    );
}

#[test]
fn signal_commands_reject_invalid_names() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(&manifest_path, manifest_toml()).unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    for args in [
        ["signal", "begin", "BadName"],
        ["signal", "clear", "release/v1"],
    ] {
        let output = run_command(&project_dir, &args);
        assert!(!output.status.success(), "command should fail");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("[a-z0-9][a-z0-9_-]*"),
            "stderr should describe the expected pattern: {stderr}"
        );
    }
}

#[test]
fn signal_commands_accept_names_with_underscores() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(
        &manifest_path,
        manifest_toml().replace("name = \"begin\"", "name = \"qa_ready\""),
    )
    .unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let output = run_command(&project_dir, &["signal", "begin", "qa_ready"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn signal_commands_fail_when_project_is_not_initialized() {
    let dir = tempfile::tempdir().unwrap();

    let output = run_command(dir.path(), &["signal", "list"]);
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("run 'runa init' first"), "stderr: {stderr}");
}

#[test]
fn signal_begin_ignores_unused_config_override() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(&manifest_path, manifest_toml()).unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let missing_config = dir.path().join("missing-config.toml");
    let output = run_command(
        &project_dir,
        &[
            "--config",
            missing_config.to_str().unwrap(),
            "signal",
            "begin",
            "foo",
        ],
    );

    assert!(
        output.status.success(),
        "signal commands should ignore unused config overrides: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stored: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(project_dir.join(".runa/signals.json")).unwrap())
            .unwrap();
    assert_eq!(stored, serde_json::json!({ "active": ["foo"] }));
}

#[test]
fn signal_begin_accepts_valid_external_config_override() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(&manifest_path, manifest_toml()).unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let external_config = dir.path().join("external-config.toml");
    let canonical_manifest = fs::canonicalize(&manifest_path).unwrap();
    fs::write(
        &external_config,
        format!(
            "methodology_path = {:?}\n",
            canonical_manifest.display().to_string()
        ),
    )
    .unwrap();

    let output = run_command(
        &project_dir,
        &[
            "--config",
            external_config.to_str().unwrap(),
            "signal",
            "begin",
            "foo",
        ],
    );

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stored: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(project_dir.join(".runa/signals.json")).unwrap())
            .unwrap();
    assert_eq!(stored, serde_json::json!({ "active": ["foo"] }));
}

#[test]
fn signal_begin_works_without_config_override_after_external_config_init() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(&manifest_path, manifest_toml()).unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();

    let external_config = dir.path().join("external-config.toml");
    init_project_with_args(
        &project_dir,
        &[
            "init",
            "--methodology",
            manifest_path.to_str().unwrap(),
            "--config",
            external_config.to_str().unwrap(),
        ],
    );

    assert!(
        !project_dir.join(".runa/config.toml").exists(),
        "project-local config should not exist when init wrote to an external path"
    );

    let output = run_command(&project_dir, &["signal", "begin", "foo"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stored: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(project_dir.join(".runa/signals.json")).unwrap())
            .unwrap();
    assert_eq!(stored, serde_json::json!({ "active": ["foo"] }));
}

#[test]
fn signal_help_reports_actual_signal_name_pattern() {
    for args in [["signal", "begin", "--help"], ["signal", "clear", "--help"]] {
        let output = runa_bin().args(args).output().unwrap();
        assert!(
            output.status.success(),
            "help should succeed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("[a-z0-9][a-z0-9_-]*"),
            "stdout should describe the validator pattern: {stdout}"
        );
    }
}
