mod common;

use std::fs;
use std::process::Command;

fn runa_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_runa"))
}

fn valid_manifest_toml() -> &'static str {
    r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"

[[protocols]]
name = "ground"
produces = ["constraints"]
trigger = { type = "on_change", name = "constraints" }
"#
}

fn constraints_schema() -> &'static str {
    r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#
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

fn append_logging_config(project_dir: &std::path::Path, logging_toml: &str) {
    let config_path = project_dir.join(".runa/config.toml");
    let existing = fs::read_to_string(&config_path).unwrap();
    fs::write(config_path, format!("{existing}\n{logging_toml}")).unwrap();
}

#[test]
fn scan_formats_output_and_succeeds_with_findings() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        valid_manifest_toml(),
        &[("constraints", constraints_schema())],
        &["ground"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::create_dir_all(workspace.join("unknown")).unwrap();
    fs::write(workspace.join("constraints/good.json"), r#"{"title":"ok"}"#).unwrap();
    fs::write(workspace.join("constraints/invalid.json"), r#"{"score":1}"#).unwrap();
    fs::write(workspace.join("constraints/bad.json"), r#"{ nope }"#).unwrap();

    let store_dir = project_dir.join(".runa/store/constraints");
    fs::create_dir_all(&store_dir).unwrap();
    let removed_state = serde_json::json!({
        "path": project_dir.join(".runa/workspace/constraints/removed.json"),
        "status": "valid",
        "last_modified_ms": 1000,
        "content_hash": "sha256:old"
    });
    fs::write(
        store_dir.join("removed.json"),
        serde_json::to_string_pretty(&removed_state).unwrap(),
    )
    .unwrap();

    let output = runa_bin()
        .arg("scan")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Summary:"), "stdout: {stdout}");
    assert!(stdout.contains("New:"), "stdout: {stdout}");
    assert!(stdout.contains("revalidated"), "stdout: {stdout}");
    assert!(stdout.contains("Invalid:"), "stdout: {stdout}");
    assert!(stdout.contains("Malformed:"), "stdout: {stdout}");
    assert!(stdout.contains("Removed:"), "stdout: {stdout}");
    assert!(
        stdout.contains("Unrecognized directories:"),
        "stdout: {stdout}"
    );
}

#[test]
fn scan_keeps_stderr_quiet_by_default_on_success() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        valid_manifest_toml(),
        &[("constraints", constraints_schema())],
        &["ground"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::write(workspace.join("constraints/good.json"), r#"{"title":"ok"}"#).unwrap();

    let output = runa_bin()
        .arg("scan")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn scan_returns_non_zero_on_workspace_io_failure() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        valid_manifest_toml(),
        &[("constraints", constraints_schema())],
        &["ground"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::remove_dir_all(&workspace).unwrap();
    fs::write(&workspace, "not a directory").unwrap();

    let output = runa_bin()
        .arg("scan")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "scan should fail on unreadable workspace"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("I/O error"), "stderr: {stderr}");
}

#[test]
fn scan_returns_non_zero_when_workspace_is_missing_and_store_has_state() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        valid_manifest_toml(),
        &[("constraints", constraints_schema())],
        &["ground"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    let store_dir = project_dir.join(".runa/store/constraints");
    fs::create_dir_all(&store_dir).unwrap();
    let stored_state = serde_json::json!({
        "path": project_dir.join(".runa/workspace/constraints/good.json"),
        "status": "valid",
        "last_modified_ms": 1000,
        "content_hash": "sha256:abc123",
        "schema_hash": "sha256:def456"
    });
    fs::write(
        store_dir.join("good.json"),
        serde_json::to_string_pretty(&stored_state).unwrap(),
    )
    .unwrap();
    fs::remove_dir_all(&workspace).unwrap();

    let output = runa_bin()
        .arg("scan")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "scan should fail when workspace is missing but store has state"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("workspace directory is missing"),
        "stderr: {stderr}"
    );
}

#[cfg(unix)]
#[test]
fn scan_reports_partially_scanned_types_and_suppresses_removals() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        valid_manifest_toml(),
        &[("constraints", constraints_schema())],
        &["ground"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    let unreadable = workspace.join("constraints/bad.json");
    fs::write(&unreadable, r#"{"title":"ok"}"#).unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o0)).unwrap();

    let store_dir = project_dir.join(".runa/store/constraints");
    fs::create_dir_all(&store_dir).unwrap();
    let kept_state = serde_json::json!({
        "path": project_dir.join(".runa/workspace/constraints/kept.json"),
        "status": "valid",
        "last_modified_ms": 1000,
        "content_hash": "sha256:abc123",
        "schema_hash": "sha256:def456"
    });
    fs::write(
        store_dir.join("kept.json"),
        serde_json::to_string_pretty(&kept_state).unwrap(),
    )
    .unwrap();

    let output = runa_bin()
        .arg("scan")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o644)).unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Partially scanned types:"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("only partially readable"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("removal suppressed for this type"),
        "stdout: {stdout}"
    );
    assert!(!stdout.contains("Removed:"), "stdout: {stdout}");
    assert!(store_dir.join("kept.json").exists());
}

#[test]
fn scan_emits_human_readable_logs_when_info_filter_is_configured() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        valid_manifest_toml(),
        &[("constraints", constraints_schema())],
        &["ground"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);
    append_logging_config(
        &project_dir,
        r#"[logging]
filter = "info"
"#,
    );

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::write(workspace.join("constraints/good.json"), r#"{"title":"ok"}"#).unwrap();

    let output = runa_bin()
        .arg("scan")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("INFO"), "stderr: {stderr}");
    assert!(stderr.contains("scan"), "stderr: {stderr}");
    assert!(stderr.contains("completed"), "stderr: {stderr}");
}

#[test]
fn scan_emits_json_logs_when_json_format_is_configured() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        valid_manifest_toml(),
        &[("constraints", constraints_schema())],
        &["ground"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);
    append_logging_config(
        &project_dir,
        r#"[logging]
format = "json"
filter = "info"
"#,
    );

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::write(workspace.join("constraints/good.json"), r#"{"title":"ok"}"#).unwrap();

    let output = runa_bin()
        .arg("scan")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    let event: serde_json::Value = serde_json::from_str(stderr.trim()).unwrap();
    assert_eq!(event["level"], "INFO");
    assert_eq!(event["fields"]["operation"], "scan");
    assert_eq!(event["fields"]["outcome"], "completed");
}

#[test]
fn scan_rust_log_overrides_config_filter() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = common::write_methodology(
        dir.path(),
        valid_manifest_toml(),
        &[("constraints", constraints_schema())],
        &["ground"],
    );

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);
    append_logging_config(
        &project_dir,
        r#"[logging]
filter = "info"
"#,
    );

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("constraints")).unwrap();
    fs::write(workspace.join("constraints/good.json"), r#"{"title":"ok"}"#).unwrap();

    let output = runa_bin()
        .arg("scan")
        .env("RUST_LOG", "error")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "RUST_LOG=error should suppress info-level events from config filter: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn scan_logs_errors_with_env_defaults_when_config_is_missing() {
    let dir = tempfile::tempdir().unwrap();

    let output = runa_bin()
        .arg("scan")
        .env("RUST_LOG", "error")
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ERROR"), "stderr: {stderr}");
    assert!(stderr.contains("command"), "stderr: {stderr}");
    assert!(stderr.contains("scan"), "stderr: {stderr}");
    assert!(stderr.contains("no config found"), "stderr: {stderr}");
}

#[test]
fn scan_logs_errors_with_env_defaults_when_config_is_malformed() {
    let dir = tempfile::tempdir().unwrap();
    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    fs::create_dir(project_dir.join(".runa")).unwrap();
    fs::write(project_dir.join(".runa/config.toml"), "{ not toml").unwrap();

    let output = runa_bin()
        .arg("scan")
        .env("RUST_LOG", "error")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ERROR"), "stderr: {stderr}");
    assert!(stderr.contains("command"), "stderr: {stderr}");
    assert!(stderr.contains("scan"), "stderr: {stderr}");
    assert!(
        stderr.contains("failed to parse config"),
        "stderr: {stderr}"
    );
}
