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
fn list_shows_skills_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(&manifest_path, valid_manifest_toml()).unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let output = runa_bin()
        .arg("list")
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
    assert!(stdout.contains("1. ground"), "stdout: {stdout}");
    assert!(stdout.contains("produces: constraints"), "stdout: {stdout}");
    assert!(stdout.contains("trigger:"), "stdout: {stdout}");

    // Verify ordering: ground before design, design before review.
    let ground_pos = stdout.find("ground").unwrap();
    let design_pos = stdout.find("design").unwrap();
    let review_pos = stdout.find("review").unwrap();
    assert!(ground_pos < design_pos, "ground should be before design");
    assert!(design_pos < review_pos, "design should be before review");
}

#[test]
fn list_shows_blocked_status() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(&manifest_path, valid_manifest_toml()).unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let output = runa_bin()
        .arg("list")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    // design requires constraints which has no valid instances → BLOCKED.
    assert!(stdout.contains("BLOCKED"), "stdout: {stdout}");
    assert!(stdout.contains("constraints"), "stdout: {stdout}");
}

#[test]
fn list_implicitly_scans_workspace_before_reporting() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("manifest.toml");
    fs::write(&manifest_path, valid_manifest_toml()).unwrap();

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);
    fs::create_dir_all(project_dir.join(".runa/workspace/constraints")).unwrap();
    fs::write(
        project_dir.join(".runa/workspace/constraints/good.json"),
        r#"{"title":"ok"}"#,
    )
    .unwrap();

    let output = runa_bin()
        .arg("list")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("missing artifact type 'constraints'"),
        "stdout: {stdout}"
    );
}

#[test]
fn list_errors_on_uninitialized_project() {
    let dir = tempfile::tempdir().unwrap();

    let output = runa_bin()
        .arg("list")
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no config found"), "stderr: {stderr}");
}
