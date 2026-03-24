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
    ("constraints", r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#),
    ("design-doc", r#"{"type":"object"}"#),
];

const PROTOCOLS: &[&str] = &["ground", "design", "review"];

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
fn doctor_healthy_exit_zero() {
    let dir = tempfile::tempdir().unwrap();
    // Use a manifest with no requires so everything is "ok".
    let manifest = r#"
name = "simple"

[[artifact_types]]
name = "report"

[[protocols]]
name = "generate"
produces = ["report"]
trigger = { type = "on_change", name = "report" }
"#;
    let manifest_path = common::write_methodology(
        dir.path(),
        manifest,
        &[("report", r#"{"type":"object"}"#)],
        &["generate"],
    );

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
    let manifest_path =
        common::write_methodology(dir.path(), valid_manifest_toml(), SCHEMAS, PROTOCOLS);

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
        "should exit 1 with unready protocols"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("cannot execute"), "stdout: {stdout}");
    assert!(stdout.contains("problem"), "stdout: {stdout}");
}

#[test]
fn doctor_implicitly_scans_workspace_before_reporting() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path =
        common::write_methodology(dir.path(), valid_manifest_toml(), SCHEMAS, PROTOCOLS);

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
        .arg("doctor")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(!output.status.success(), "review should still be unready");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("design: ok"), "stdout: {stdout}");
}

#[test]
fn doctor_with_invalid_artifacts_exit_one() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path =
        common::write_methodology(dir.path(), valid_manifest_toml(), SCHEMAS, PROTOCOLS);

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    fs::create_dir_all(project_dir.join(".runa/workspace/constraints")).unwrap();
    fs::write(
        project_dir.join(".runa/workspace/constraints/bad.json"),
        r#"{"score":1}"#,
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
    let manifest_path =
        common::write_methodology(dir.path(), valid_manifest_toml(), SCHEMAS, PROTOCOLS);

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    fs::create_dir_all(project_dir.join(".runa/workspace/constraints")).unwrap();
    fs::write(
        project_dir.join(".runa/workspace/constraints/bad.json"),
        r#"{ nope }"#,
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

#[cfg(unix)]
#[test]
fn doctor_reports_unreadable_workspace_entries_as_problems() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path =
        common::write_methodology(dir.path(), valid_manifest_toml(), SCHEMAS, PROTOCOLS);

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let artifact_dir = project_dir.join(".runa/workspace/constraints");
    fs::create_dir_all(&artifact_dir).unwrap();
    let unreadable = artifact_dir.join("bad.json");
    fs::write(&unreadable, r#"{"title":"ok"}"#).unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o0)).unwrap();

    let output = runa_bin()
        .arg("doctor")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o644)).unwrap();

    assert!(
        !output.status.success(),
        "should exit 1 with unreadable workspace entries"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Scan:"), "stdout: {stdout}");
    assert!(
        stdout.contains("only partially readable"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("unreadable:"), "stdout: {stdout}");
    assert!(
        stdout.contains("removal suppressed for this type"),
        "stdout: {stdout}"
    );
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

#[test]
fn doctor_reports_stale_required_artifacts_as_stale() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path =
        common::write_methodology(dir.path(), valid_manifest_toml(), SCHEMAS, PROTOCOLS);

    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);
    fs::create_dir_all(project_dir.join(".runa/workspace/constraints")).unwrap();
    fs::write(
        project_dir.join(".runa/workspace/constraints/good.json"),
        r#"{"title":"ok"}"#,
    )
    .unwrap();

    scan_project(&project_dir);

    let store_path = project_dir.join(".runa/store/constraints/good.json");
    let mut state: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&store_path).unwrap()).unwrap();
    state["status"] = serde_json::json!("stale");
    fs::write(&store_path, serde_json::to_string_pretty(&state).unwrap()).unwrap();

    let output = runa_bin()
        .arg("doctor")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "stale required artifacts should block doctor"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("stale: constraints"), "stdout: {stdout}");
    assert!(!stdout.contains("missing: constraints"), "stdout: {stdout}");
}
