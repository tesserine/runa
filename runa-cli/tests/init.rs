mod common;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

fn runa_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_runa"))
}

fn valid_manifest_toml() -> &'static str {
    r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"

[[artifact_types]]
name = "design-doc"

[[protocols]]
name = "ground"
produces = ["constraints"]
trigger = { type = "on_change", name = "constraints" }

[[protocols]]
name = "design"
requires = ["constraints"]
produces = ["design-doc"]
trigger = { type = "on_artifact", name = "constraints" }

[[protocols]]
name = "review"
requires = ["design-doc"]
trigger = { type = "on_artifact", name = "design-doc" }
"#
}

const SCHEMAS: &[(&str, &str)] = &[
    ("constraints", r#"{"type":"object"}"#),
    ("design-doc", r#"{"type":"object"}"#),
];

const PROTOCOLS: &[&str] = &["ground", "design", "review"];

#[test]
fn init_creates_runa_directory() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path =
        common::write_methodology(dir.path(), valid_manifest_toml(), SCHEMAS, PROTOCOLS);

    let project_dir = dir.path().join("project");
    std::fs::create_dir(&project_dir).unwrap();

    let output = runa_bin()
        .arg("init")
        .arg("--methodology")
        .arg(&manifest_path)
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("groundwork"), "stdout: {stdout}");
    assert!(stdout.contains("2 artifact types"), "stdout: {stdout}");
    assert!(stdout.contains("3 protocols"), "stdout: {stdout}");

    assert!(project_dir.join(".runa").is_dir());
    assert!(project_dir.join(".runa/state.toml").is_file());

    assert!(project_dir.join(".runa/store").is_dir());
    assert!(project_dir.join(".runa/workspace").is_dir());
}

#[test]
fn init_fails_with_nonexistent_methodology() {
    let dir = tempfile::tempdir().unwrap();

    let output = runa_bin()
        .arg("init")
        .arg("--methodology")
        .arg(dir.path().join("no-such-file.toml"))
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("methodology not found"), "stderr: {stderr}");
}

#[test]
fn init_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path =
        common::write_methodology(dir.path(), valid_manifest_toml(), SCHEMAS, PROTOCOLS);

    let project_dir = dir.path().join("project");
    std::fs::create_dir(&project_dir).unwrap();

    let output1 = runa_bin()
        .arg("init")
        .arg("--methodology")
        .arg(&manifest_path)
        .current_dir(&project_dir)
        .output()
        .unwrap();
    assert!(output1.status.success());

    let output2 = runa_bin()
        .arg("init")
        .arg("--methodology")
        .arg(&manifest_path)
        .current_dir(&project_dir)
        .output()
        .unwrap();
    assert!(output2.status.success());

    let stdout = String::from_utf8_lossy(&output2.stdout);
    assert!(stdout.contains("groundwork"));
}

#[test]
fn init_rejects_removed_artifacts_dir_flag() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path =
        common::write_methodology(dir.path(), valid_manifest_toml(), SCHEMAS, PROTOCOLS);

    let project_dir = dir.path().join("project");
    std::fs::create_dir(&project_dir).unwrap();

    let output = runa_bin()
        .arg("init")
        .arg("--methodology")
        .arg(&manifest_path)
        .arg("--artifacts-dir")
        .arg("custom-artifacts")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--artifacts-dir"), "stderr: {stderr}");
}

#[cfg(unix)]
#[test]
fn init_reports_actionable_error_for_unwritable_existing_runa_directory() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path =
        common::write_methodology(dir.path(), valid_manifest_toml(), SCHEMAS, PROTOCOLS);

    let project_dir = dir.path().join("project");
    std::fs::create_dir(&project_dir).unwrap();
    let runa_dir = project_dir.join(".runa");
    std::fs::create_dir(&runa_dir).unwrap();
    std::fs::set_permissions(&runa_dir, std::fs::Permissions::from_mode(0o500)).unwrap();

    let output = runa_bin()
        .arg("init")
        .arg("--methodology")
        .arg(&manifest_path)
        .current_dir(&project_dir)
        .output()
        .unwrap();

    std::fs::set_permissions(&runa_dir, std::fs::Permissions::from_mode(0o700)).unwrap();

    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(".runa"), "stderr: {stderr}");
    assert!(stderr.contains("owned by uid"), "stderr: {stderr}");
    assert!(stderr.contains("current uid"), "stderr: {stderr}");
    assert!(stderr.contains("not writable"), "stderr: {stderr}");
    assert!(
        stderr.contains("managed by another tool"),
        "stderr: {stderr}"
    );
    assert!(!stderr.contains("agentd"), "stderr: {stderr}");
    assert!(stderr.contains("remove"), "stderr: {stderr}");
    assert!(
        !stderr.contains("Permission denied (os error 13)"),
        "stderr: {stderr}"
    );
}

#[cfg(unix)]
#[test]
fn init_reports_actionable_error_for_unwritable_existing_config_file() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path =
        common::write_methodology(dir.path(), valid_manifest_toml(), SCHEMAS, PROTOCOLS);

    let project_dir = dir.path().join("project");
    std::fs::create_dir(&project_dir).unwrap();
    let runa_dir = project_dir.join(".runa");
    std::fs::create_dir(&runa_dir).unwrap();
    let config_path = runa_dir.join("config.toml");
    std::fs::write(&config_path, "old config").unwrap();
    std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o400)).unwrap();

    let output = runa_bin()
        .arg("init")
        .arg("--methodology")
        .arg(&manifest_path)
        .current_dir(&project_dir)
        .output()
        .unwrap();

    std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600)).unwrap();

    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(".runa/config.toml"), "stderr: {stderr}");
    assert!(stderr.contains("not writable"), "stderr: {stderr}");
    assert!(
        stderr.contains("managed by another tool"),
        "stderr: {stderr}"
    );
    assert!(!stderr.contains("agentd"), "stderr: {stderr}");
    assert!(
        !stderr.contains("Permission denied (os error 13)"),
        "stderr: {stderr}"
    );
    assert!(
        !runa_dir.join("store").exists(),
        "store should not be created before the diagnostic"
    );
    assert!(
        !runa_dir.join("workspace").exists(),
        "workspace should not be created before the diagnostic"
    );
    assert!(
        !runa_dir.join("state.toml").exists(),
        "state should not be created before the diagnostic"
    );
}

#[cfg(unix)]
#[test]
fn init_reports_actionable_error_for_unwritable_custom_config_file_before_creating_runa_dir() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path =
        common::write_methodology(dir.path(), valid_manifest_toml(), SCHEMAS, PROTOCOLS);

    let project_dir = dir.path().join("project");
    std::fs::create_dir(&project_dir).unwrap();
    let custom_config_dir = dir.path().join("custom");
    std::fs::create_dir(&custom_config_dir).unwrap();
    let custom_config_path = custom_config_dir.join("config.toml");
    std::fs::write(&custom_config_path, "old config").unwrap();
    std::fs::set_permissions(&custom_config_path, std::fs::Permissions::from_mode(0o400)).unwrap();

    let output = runa_bin()
        .arg("--config")
        .arg(&custom_config_path)
        .arg("init")
        .arg("--methodology")
        .arg(&manifest_path)
        .current_dir(&project_dir)
        .output()
        .unwrap();

    std::fs::set_permissions(&custom_config_path, std::fs::Permissions::from_mode(0o600)).unwrap();

    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(custom_config_path.to_string_lossy().as_ref()),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("not writable"), "stderr: {stderr}");
    assert!(
        !stderr.contains("Permission denied (os error 13)"),
        "stderr: {stderr}"
    );
    assert!(
        !project_dir.join(".runa").exists(),
        ".runa should not be created before the diagnostic"
    );
}
