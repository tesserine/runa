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
schema = { type = "object" }

[[artifact_types]]
name = "design-doc"
schema = { type = "object" }

[[skills]]
name = "ground"
produces = ["constraints"]
trigger = { type = "on_signal", name = "init" }

[[skills]]
name = "design"
requires = ["constraints"]
produces = ["design-doc"]
trigger = { type = "on_artifact", name = "constraints" }

[[skills]]
name = "review"
requires = ["design-doc"]
trigger = { type = "on_artifact", name = "design-doc" }
"#
}

#[test]
fn init_creates_runa_directory() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(&manifest_path, valid_manifest_toml()).unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();

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
    assert!(stdout.contains("3 skills"), "stdout: {stdout}");

    assert!(project_dir.join(".runa").is_dir());
    let state = fs::read_to_string(project_dir.join(".runa/state.toml")).unwrap();
    assert!(state.contains("groundwork"));
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
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(&manifest_path, valid_manifest_toml()).unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();

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
