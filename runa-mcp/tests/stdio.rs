use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use rmcp::model::CallToolRequestParam;
use rmcp::service::ServiceExt;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use tokio::process::Command;

fn write_methodology(
    dir: &Path,
    manifest_toml: &str,
    schemas: &[(&str, &str)],
    protocols: &[&str],
) -> PathBuf {
    let manifest_path = dir.join("manifest.toml");
    fs::write(&manifest_path, manifest_toml).unwrap();

    let schemas_dir = dir.join("schemas");
    fs::create_dir_all(&schemas_dir).unwrap();
    for (name, content) in schemas {
        fs::write(schemas_dir.join(format!("{name}.schema.json")), content).unwrap();
    }

    for protocol_name in protocols {
        let protocol_dir = dir.join("protocols").join(protocol_name);
        fs::create_dir_all(&protocol_dir).unwrap();
        fs::write(
            protocol_dir.join("PROTOCOL.md"),
            format!("# {protocol_name}\n"),
        )
        .unwrap();
    }

    manifest_path
}

fn manifest_toml() -> &'static str {
    r#"
name = "groundwork"

[[artifact_types]]
name = "summary"

[[artifact_types]]
name = "implementation"

[[protocols]]
name = "summarize"
produces = ["summary"]
trigger = { type = "on_change", name = "summary" }

[[protocols]]
name = "implement"
produces = ["implementation"]
scoped = true
trigger = { type = "on_change", name = "implementation" }
"#
}

fn methodology_schemas() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "summary",
            r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
        ),
        (
            "implementation",
            r#"{"type":"object","required":["title","work_unit"],"properties":{"title":{"type":"string"},"work_unit":{"type":"string"}}}"#,
        ),
    ]
}

fn methodology_protocols() -> Vec<&'static str> {
    vec!["summarize", "implement"]
}

fn init_project(project_dir: &Path, manifest_path: &Path) {
    let runa_dir = project_dir.join(".runa");
    fs::create_dir_all(&runa_dir).unwrap();

    let manifest_path = fs::canonicalize(manifest_path).unwrap();
    fs::write(
        runa_dir.join("config.toml"),
        format!(
            "methodology_path = {:?}\n",
            manifest_path.display().to_string()
        ),
    )
    .unwrap();
    fs::write(
        runa_dir.join("state.toml"),
        "initialized_at = \"2026-03-25T00:00:00Z\"\nruna_version = \"0.1.0\"\n",
    )
    .unwrap();
}

fn setup_project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        manifest_toml(),
        &methodology_schemas(),
        &methodology_protocols(),
    );
    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);
    dir
}

#[test]
fn missing_protocol_argument_fails_clearly() {
    let dir = tempfile::tempdir().unwrap();
    let output = StdCommand::new(env!("CARGO_BIN_EXE_runa-mcp"))
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--protocol"), "stderr: {stderr}");
}

#[test]
fn unknown_protocol_name_references_manifest() {
    let dir = setup_project();
    let project_dir = dir.path().join("project");

    let output = StdCommand::new(env!("CARGO_BIN_EXE_runa-mcp"))
        .arg("--protocol")
        .arg("missing")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("missing"), "stderr: {stderr}");
    assert!(stderr.contains("manifest"), "stderr: {stderr}");
    assert!(stderr.contains("groundwork"), "stderr: {stderr}");
}

#[tokio::test]
async fn starts_and_serves_tools_without_workspace_directory() {
    let dir = setup_project();
    let project_dir = dir.path().join("project");
    let workspace_dir = project_dir.join(".runa/workspace");
    assert!(!workspace_dir.exists());

    let service = ()
        .serve(
            TokioChildProcess::new(
                Command::new(env!("CARGO_BIN_EXE_runa-mcp")).configure(|cmd| {
                    cmd.arg("--protocol")
                        .arg("summarize")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let tools = service.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name.as_ref(), "summary");

    service.cancel().await.unwrap();
}

#[tokio::test]
async fn scoped_protocol_writes_artifact_with_injected_work_unit() {
    let dir = setup_project();
    let project_dir = dir.path().join("project");

    let service = ()
        .serve(
            TokioChildProcess::new(
                Command::new(env!("CARGO_BIN_EXE_runa-mcp")).configure(|cmd| {
                    cmd.arg("--protocol")
                        .arg("implement")
                        .arg("--work-unit")
                        .arg("wu-1")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let tools = service.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name.as_ref(), "implementation");

    service
        .call_tool(CallToolRequestParam {
            name: "implementation".into(),
            arguments: serde_json::json!({
                "instance_id": "impl-1",
                "title": "ship it"
            })
            .as_object()
            .cloned(),
        })
        .await
        .unwrap();

    let artifact =
        fs::read_to_string(project_dir.join(".runa/workspace/implementation/impl-1.json")).unwrap();
    assert!(artifact.contains("\"title\": \"ship it\""), "{artifact}");
    assert!(artifact.contains("\"work_unit\": \"wu-1\""), "{artifact}");

    service.cancel().await.unwrap();
}
