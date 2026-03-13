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
schema = { type = "object", required = ["title"], properties = { title = { type = "string" } } }

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
fn doctor_healthy_exit_zero() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    // Use a manifest with no requires so everything is "ok".
    let manifest = r#"
name = "simple"

[[artifact_types]]
name = "report"
schema = { type = "object" }

[[skills]]
name = "generate"
produces = ["report"]
trigger = { type = "on_signal", name = "go" }
"#;
    fs::write(&manifest_path, manifest).unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let output = runa_bin()
        .arg("doctor")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("No problems found"), "stdout: {stdout}");
}

#[test]
fn doctor_unready_skills_exit_one() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(&manifest_path, valid_manifest_toml()).unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let output = runa_bin()
        .arg("doctor")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "should exit 1 with unready skills"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("cannot execute"), "stdout: {stdout}");
    assert!(stdout.contains("problem"), "stdout: {stdout}");
}

#[test]
fn doctor_with_invalid_artifacts_exit_one() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(&manifest_path, valid_manifest_toml()).unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    // Write an invalid artifact state file directly.
    let artifact_dir = project_dir.join(".runa/store/constraints");
    fs::create_dir_all(&artifact_dir).unwrap();
    let invalid_state = serde_json::json!({
        "path": "c.json",
        "status": {
            "invalid": [
                {
                    "artifact_type": "constraints",
                    "description": "missing required field 'title'",
                    "schema_path": "/required",
                    "instance_path": ""
                }
            ]
        },
        "last_modified_ms": 1000,
        "content_hash": "sha256:abc123"
    });
    fs::write(
        artifact_dir.join("bad.json"),
        serde_json::to_string_pretty(&invalid_state).unwrap(),
    )
    .unwrap();

    let output = runa_bin()
        .arg("doctor")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "should exit 1 with invalid artifacts"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("invalid"), "stdout: {stdout}");
}

#[test]
fn doctor_with_malformed_artifacts_exit_one() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(&manifest_path, valid_manifest_toml()).unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let artifact_dir = project_dir.join(".runa/store/constraints");
    fs::create_dir_all(&artifact_dir).unwrap();
    let malformed_state = serde_json::json!({
        "path": "c.json",
        "status": {
            "malformed": "expected value at line 1 column 1"
        },
        "last_modified_ms": 1000,
        "content_hash": "sha256:abc123"
    });
    fs::write(
        artifact_dir.join("bad.json"),
        serde_json::to_string_pretty(&malformed_state).unwrap(),
    )
    .unwrap();

    let output = runa_bin()
        .arg("doctor")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "should exit 1 with malformed artifacts"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("malformed"), "stdout: {stdout}");
}

#[test]
fn doctor_errors_on_uninitialized_project() {
    let dir = tempfile::tempdir().unwrap();

    let output = runa_bin()
        .arg("doctor")
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no config found"), "stderr: {stderr}");
}
