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

fn required_choice_with_unsupported_optional_manifest_toml() -> &'static str {
    r#"
name = "groundwork"

[[artifact_types]]
name = "approved"

[[artifact_types]]
name = "needs-revision"

[[artifact_types]]
name = "audit-log"

[[protocols]]
name = "review"
may_produce = ["audit-log"]
trigger = { type = "on_change", name = "approved" }

[[protocols.required_output_choices]]
name = "disposition"
members = ["approved", "needs-revision"]
"#
}

fn required_choice_with_unsupported_optional_schemas() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "approved",
            r#"{"type":"object","required":["summary"],"properties":{"summary":{"type":"string"}}}"#,
        ),
        (
            "needs-revision",
            r#"{"type":"object","required":["summary"],"properties":{"summary":{"type":"string"}}}"#,
        ),
        ("audit-log", r#"{"type":"array","items":{"type":"string"}}"#),
    ]
}

fn scoped_work_unit_manifest_toml() -> &'static str {
    r#"
name = "groundwork"

[[artifact_types]]
name = "work-unit"

[[artifact_types]]
name = "claim"

[[protocols]]
name = "take"
requires = ["work-unit"]
produces = ["claim"]
scoped = true
trigger = { type = "on_artifact", name = "work-unit" }
"#
}

fn scoped_work_unit_schemas() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "work-unit",
            r#"{"type":"object","required":["title","description","acceptance_criteria"],"properties":{"title":{"type":"string"},"description":{"type":"string"},"acceptance_criteria":{"type":"array","items":{"type":"string"}},"handle":{"type":"object"}}}"#,
        ),
        (
            "claim",
            r#"{"type":"object","required":["work_unit","scope"],"properties":{"work_unit":{"type":"string"},"scope":{"type":"string"}}}"#,
        ),
    ]
}

fn scoped_multi_protocol_manifest_toml() -> &'static str {
    r#"
name = "groundwork"

[[artifact_types]]
name = "work-unit"

[[artifact_types]]
name = "claim"

[[artifact_types]]
name = "implementation"

[[protocols]]
name = "take"
requires = ["work-unit"]
produces = ["claim"]
scoped = true
trigger = { type = "on_artifact", name = "work-unit" }

[[protocols]]
name = "implement"
requires = ["claim"]
produces = ["implementation"]
scoped = true
trigger = { type = "on_artifact", name = "claim" }
"#
}

fn scoped_multi_protocol_schemas() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "work-unit",
            r#"{"type":"object","required":["title","description","acceptance_criteria"],"properties":{"title":{"type":"string"},"description":{"type":"string"},"acceptance_criteria":{"type":"array","items":{"type":"string"}},"handle":{"type":"object"}}}"#,
        ),
        (
            "claim",
            r#"{"type":"object","required":["work_unit","scope"],"properties":{"work_unit":{"type":"string"},"scope":{"type":"string"}}}"#,
        ),
        (
            "implementation",
            r#"{"type":"object","required":["work_unit","title"],"properties":{"work_unit":{"type":"string"},"title":{"type":"string"}}}"#,
        ),
    ]
}

fn scoped_claim_to_implementation_manifest_toml() -> &'static str {
    r#"
name = "groundwork"

[[artifact_types]]
name = "work-unit"

[[artifact_types]]
name = "claim"

[[artifact_types]]
name = "implementation"

[[protocols]]
name = "implement"
requires = ["claim"]
produces = ["implementation"]
scoped = true
trigger = { type = "on_artifact", name = "claim" }
"#
}

fn github_work_unit_json(number: u64) -> String {
    format!(
        r#"{{"title":"Scope","description":"Enforce canonical scope","acceptance_criteria":["Reject aliases"],"handle":{{"forge_tag":"github","url":"https://github.com/tesserine/runa/issues/{number}","number":{number}}}}}"#
    )
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

fn text_content(result: &rmcp::model::CallToolResult) -> &str {
    result.content[0]
        .as_text()
        .expect("tool result should contain text")
        .text
        .as_str()
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
fn missing_protocol_and_work_unit_arguments_fail_clearly() {
    let dir = tempfile::tempdir().unwrap();
    let output = StdCommand::new(env!("CARGO_BIN_EXE_runa-mcp"))
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--work-unit"), "stderr: {stderr}");
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
async fn choice_only_protocol_with_unsupported_may_produce_starts() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        required_choice_with_unsupported_optional_manifest_toml(),
        &required_choice_with_unsupported_optional_schemas(),
        &["review"],
    );
    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let service = ()
        .serve(
            TokioChildProcess::new(
                Command::new(env!("CARGO_BIN_EXE_runa-mcp")).configure(|cmd| {
                    cmd.arg("--protocol")
                        .arg("review")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let tools = service.list_all_tools().await.unwrap();
    let tool_names = tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();
    assert_eq!(tool_names, vec!["approved", "needs-revision"]);

    service.cancel().await.unwrap();
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
async fn session_surface_advertises_driver_verbs_and_ready_output_tools() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        scoped_work_unit_manifest_toml(),
        &scoped_work_unit_schemas(),
        &["take"],
    );
    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("work-unit")).unwrap();
    fs::write(
        workspace.join("work-unit/work-unit-166.json"),
        github_work_unit_json(166),
    )
    .unwrap();

    let service = ()
        .serve(
            TokioChildProcess::new(
                Command::new(env!("CARGO_BIN_EXE_runa-mcp")).configure(|cmd| {
                    cmd.arg("--work-unit")
                        .arg("work-unit-166")
                        .env_remove("GROUNDWORK_FORGE_TYPE")
                        .env_remove("GROUNDWORK_FORGE_TRACKER_ID")
                        .env("GROUNDWORK_FORGE_OWNER", "tesserine")
                        .env("GROUNDWORK_FORGE_NAME", "runa")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let tool_names = service
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|tool| tool.name.into_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        tool_names,
        vec!["readiness", "next-protocol-context", "advance", "claim"]
    );

    service.cancel().await.unwrap();
}

#[tokio::test]
async fn readiness_reports_scoped_protocol_state_from_session_surface() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        scoped_work_unit_manifest_toml(),
        &scoped_work_unit_schemas(),
        &["take"],
    );
    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("work-unit")).unwrap();
    fs::write(
        workspace.join("work-unit/work-unit-166.json"),
        github_work_unit_json(166),
    )
    .unwrap();

    let service = ()
        .serve(
            TokioChildProcess::new(
                Command::new(env!("CARGO_BIN_EXE_runa-mcp")).configure(|cmd| {
                    cmd.arg("--work-unit")
                        .arg("work-unit-166")
                        .env_remove("GROUNDWORK_FORGE_TYPE")
                        .env_remove("GROUNDWORK_FORGE_TRACKER_ID")
                        .env("GROUNDWORK_FORGE_OWNER", "tesserine")
                        .env("GROUNDWORK_FORGE_NAME", "runa")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let result = service
        .call_tool(CallToolRequestParam {
            name: "readiness".into(),
            arguments: None,
        })
        .await
        .unwrap();

    assert_eq!(result.is_error, Some(false));
    let payload: serde_json::Value = serde_json::from_str(text_content(&result)).unwrap();
    assert_eq!(payload["version"], 1);
    assert_eq!(payload["methodology"], "groundwork");
    assert_eq!(payload["protocols"][0]["name"], "take");
    assert_eq!(payload["protocols"][0]["work_unit"], "work-unit-166");
    assert_eq!(payload["protocols"][0]["status"], "ready");

    service.cancel().await.unwrap();
}

#[tokio::test]
async fn next_protocol_context_returns_structured_context_and_rendered_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        scoped_work_unit_manifest_toml(),
        &scoped_work_unit_schemas(),
        &["take"],
    );
    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("work-unit")).unwrap();
    fs::write(
        workspace.join("work-unit/work-unit-166.json"),
        github_work_unit_json(166),
    )
    .unwrap();

    let service = ()
        .serve(
            TokioChildProcess::new(
                Command::new(env!("CARGO_BIN_EXE_runa-mcp")).configure(|cmd| {
                    cmd.arg("--work-unit")
                        .arg("work-unit-166")
                        .env_remove("GROUNDWORK_FORGE_TYPE")
                        .env_remove("GROUNDWORK_FORGE_TRACKER_ID")
                        .env("GROUNDWORK_FORGE_OWNER", "tesserine")
                        .env("GROUNDWORK_FORGE_NAME", "runa")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let result = service
        .call_tool(CallToolRequestParam {
            name: "next-protocol-context".into(),
            arguments: None,
        })
        .await
        .unwrap();

    assert_eq!(result.is_error, Some(false));
    let payload: serde_json::Value = serde_json::from_str(text_content(&result)).unwrap();
    assert_eq!(payload["protocol"], "take");
    assert_eq!(payload["work_unit"], "work-unit-166");
    assert_eq!(payload["context"]["protocol"], "take");
    assert_eq!(payload["context"]["work_unit"], "work-unit-166");
    assert_eq!(
        payload["context"]["expected_outputs"]["produces"][0],
        "claim"
    );
    assert!(
        payload["rendered_prompt"]
            .as_str()
            .unwrap()
            .contains("# Protocol: take (work_unit=work-unit-166)")
    );

    service.cancel().await.unwrap();
}

#[tokio::test]
async fn session_surface_output_tool_then_advance_records_execution() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        scoped_work_unit_manifest_toml(),
        &scoped_work_unit_schemas(),
        &["take"],
    );
    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("work-unit")).unwrap();
    fs::write(
        workspace.join("work-unit/work-unit-166.json"),
        github_work_unit_json(166),
    )
    .unwrap();

    let service = ()
        .serve(
            TokioChildProcess::new(
                Command::new(env!("CARGO_BIN_EXE_runa-mcp")).configure(|cmd| {
                    cmd.arg("--work-unit")
                        .arg("work-unit-166")
                        .env_remove("GROUNDWORK_FORGE_TYPE")
                        .env_remove("GROUNDWORK_FORGE_TRACKER_ID")
                        .env("GROUNDWORK_FORGE_OWNER", "tesserine")
                        .env("GROUNDWORK_FORGE_NAME", "runa")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let context = service
        .call_tool(CallToolRequestParam {
            name: "next-protocol-context".into(),
            arguments: None,
        })
        .await
        .unwrap();
    assert_eq!(context.is_error, Some(false));

    let produced = service
        .call_tool(CallToolRequestParam {
            name: "claim".into(),
            arguments: serde_json::json!({
                "instance_id": "claim-1",
                "scope": "Implement the unified session surface"
            })
            .as_object()
            .cloned(),
        })
        .await
        .unwrap();
    assert_eq!(
        produced.is_error,
        Some(false),
        "{}",
        text_content(&produced)
    );

    let artifact = fs::read_to_string(project_dir.join(".runa/workspace/claim/claim-1.json"))
        .expect("claim artifact should be written");
    assert!(artifact.contains("\"scope\": \"Implement the unified session surface\""));
    assert!(artifact.contains("\"work_unit\": \"work-unit-166\""));
    assert!(!artifact.contains("instance_id"));

    let advanced = service
        .call_tool(CallToolRequestParam {
            name: "advance".into(),
            arguments: None,
        })
        .await
        .unwrap();
    assert_eq!(
        advanced.is_error,
        Some(false),
        "{}",
        text_content(&advanced)
    );
    assert!(
        project_dir
            .join(".runa/store/execution-records.json")
            .exists(),
        "advance should persist execution metadata"
    );
    let payload: serde_json::Value = serde_json::from_str(text_content(&advanced)).unwrap();
    assert_eq!(payload["protocols"][0]["name"], "take");
    assert_eq!(payload["protocols"][0]["status"], "waiting");

    service.cancel().await.unwrap();
}

#[tokio::test]
async fn session_surface_output_tools_track_pending_protocol_after_advance() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        scoped_multi_protocol_manifest_toml(),
        &scoped_multi_protocol_schemas(),
        &["take", "implement"],
    );
    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("work-unit")).unwrap();
    fs::write(
        workspace.join("work-unit/work-unit-166.json"),
        github_work_unit_json(166),
    )
    .unwrap();

    let service = ()
        .serve(
            TokioChildProcess::new(
                Command::new(env!("CARGO_BIN_EXE_runa-mcp")).configure(|cmd| {
                    cmd.arg("--work-unit")
                        .arg("work-unit-166")
                        .env_remove("GROUNDWORK_FORGE_TYPE")
                        .env_remove("GROUNDWORK_FORGE_TRACKER_ID")
                        .env("GROUNDWORK_FORGE_OWNER", "tesserine")
                        .env("GROUNDWORK_FORGE_NAME", "runa")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let initial_tools = service
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|tool| tool.name.into_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        initial_tools,
        vec!["readiness", "next-protocol-context", "advance", "claim"]
    );

    let produced = service
        .call_tool(CallToolRequestParam {
            name: "claim".into(),
            arguments: serde_json::json!({
                "instance_id": "claim-1",
                "scope": "Implement the unified session surface"
            })
            .as_object()
            .cloned(),
        })
        .await
        .unwrap();
    assert_eq!(
        produced.is_error,
        Some(false),
        "{}",
        text_content(&produced)
    );

    let advanced = service
        .call_tool(CallToolRequestParam {
            name: "advance".into(),
            arguments: None,
        })
        .await
        .unwrap();
    assert_eq!(
        advanced.is_error,
        Some(false),
        "{}",
        text_content(&advanced)
    );

    let post_advance_tools = service
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|tool| tool.name.into_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        post_advance_tools,
        vec![
            "readiness",
            "next-protocol-context",
            "advance",
            "implementation"
        ]
    );

    let implementation = service
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
    assert_eq!(
        implementation.is_error,
        Some(false),
        "{}",
        text_content(&implementation)
    );

    let artifact =
        fs::read_to_string(project_dir.join(".runa/workspace/implementation/impl-1.json")).unwrap();
    assert!(artifact.contains("\"work_unit\": \"work-unit-166\""));

    service.cancel().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn session_startup_tools_honor_pre_refresh_scan_gap() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        scoped_claim_to_implementation_manifest_toml(),
        &scoped_multi_protocol_schemas(),
        &["implement"],
    );
    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("work-unit")).unwrap();
    fs::write(
        workspace.join("work-unit/work-unit-166.json"),
        github_work_unit_json(166),
    )
    .unwrap();
    fs::create_dir_all(workspace.join("claim")).unwrap();
    let claim_path = workspace.join("claim/claim-1.json");
    fs::write(
        &claim_path,
        r#"{"work_unit":"work-unit-166","scope":"Implement the unified session surface"}"#,
    )
    .unwrap();

    let initial_service = ()
        .serve(
            TokioChildProcess::new(
                Command::new(env!("CARGO_BIN_EXE_runa-mcp")).configure(|cmd| {
                    cmd.arg("--work-unit")
                        .arg("work-unit-166")
                        .env_remove("GROUNDWORK_FORGE_TYPE")
                        .env_remove("GROUNDWORK_FORGE_TRACKER_ID")
                        .env("GROUNDWORK_FORGE_OWNER", "tesserine")
                        .env("GROUNDWORK_FORGE_NAME", "runa")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    initial_service.cancel().await.unwrap();

    fs::set_permissions(&claim_path, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(&claim_path).is_ok() {
        fs::set_permissions(&claim_path, fs::Permissions::from_mode(0o644)).unwrap();
        return;
    }

    let service = ()
        .serve(
            TokioChildProcess::new(
                Command::new(env!("CARGO_BIN_EXE_runa-mcp")).configure(|cmd| {
                    cmd.arg("--work-unit")
                        .arg("work-unit-166")
                        .env_remove("GROUNDWORK_FORGE_TYPE")
                        .env_remove("GROUNDWORK_FORGE_TRACKER_ID")
                        .env("GROUNDWORK_FORGE_OWNER", "tesserine")
                        .env("GROUNDWORK_FORGE_NAME", "runa")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let tool_names = service
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|tool| tool.name.into_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        tool_names,
        vec!["readiness", "next-protocol-context", "advance"]
    );

    let readiness = service
        .call_tool(CallToolRequestParam {
            name: "readiness".into(),
            arguments: None,
        })
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_str(text_content(&readiness)).unwrap();
    assert_eq!(payload["protocols"][0]["name"], "implement");
    assert_eq!(payload["protocols"][0]["status"], "blocked");
    assert_eq!(
        payload["protocols"][0]["precondition_failures"][0]["reason"],
        "scan_incomplete"
    );

    service.cancel().await.unwrap();
    fs::set_permissions(&claim_path, fs::Permissions::from_mode(0o644)).unwrap();
}

#[test]
fn session_start_rejects_unsupported_required_output_schema() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "work-unit"

[[artifact_types]]
name = "audit-log"

[[protocols]]
name = "audit"
requires = ["work-unit"]
produces = ["audit-log"]
scoped = true
trigger = { type = "on_artifact", name = "work-unit" }
"#,
        &[
            (
                "work-unit",
                r#"{"type":"object","required":["title","description","acceptance_criteria"],"properties":{"title":{"type":"string"},"description":{"type":"string"},"acceptance_criteria":{"type":"array","items":{"type":"string"}},"handle":{"type":"object"}}}"#,
            ),
            ("audit-log", r#"{"type":"array","items":{"type":"string"}}"#),
        ],
        &["audit"],
    );
    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("work-unit")).unwrap();
    fs::write(
        workspace.join("work-unit/work-unit-166.json"),
        github_work_unit_json(166),
    )
    .unwrap();

    let output = StdCommand::new(env!("CARGO_BIN_EXE_runa-mcp"))
        .arg("--work-unit")
        .arg("work-unit-166")
        .env_remove("GROUNDWORK_FORGE_TYPE")
        .env_remove("GROUNDWORK_FORGE_TRACKER_ID")
        .env("GROUNDWORK_FORGE_OWNER", "tesserine")
        .env("GROUNDWORK_FORGE_NAME", "runa")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("required output type 'audit-log': non-object schema root type 'array'"),
        "stderr: {stderr}"
    );
}

#[test]
fn session_start_rejects_reserved_driver_verb_output_name() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "work-unit"

[[artifact_types]]
name = "advance"

[[protocols]]
name = "take"
requires = ["work-unit"]
produces = ["advance"]
scoped = true
trigger = { type = "on_artifact", name = "work-unit" }
"#,
        &[
            (
                "work-unit",
                r#"{"type":"object","required":["title","description","acceptance_criteria"],"properties":{"title":{"type":"string"},"description":{"type":"string"},"acceptance_criteria":{"type":"array","items":{"type":"string"}},"handle":{"type":"object"}}}"#,
            ),
            (
                "advance",
                r#"{"type":"object","required":["work_unit","summary"],"properties":{"work_unit":{"type":"string"},"summary":{"type":"string"}}}"#,
            ),
        ],
        &["take"],
    );
    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("work-unit")).unwrap();
    fs::write(
        workspace.join("work-unit/work-unit-166.json"),
        github_work_unit_json(166),
    )
    .unwrap();

    let output = StdCommand::new(env!("CARGO_BIN_EXE_runa-mcp"))
        .arg("--work-unit")
        .arg("work-unit-166")
        .env_remove("GROUNDWORK_FORGE_TYPE")
        .env_remove("GROUNDWORK_FORGE_TRACKER_ID")
        .env("GROUNDWORK_FORGE_OWNER", "tesserine")
        .env("GROUNDWORK_FORGE_NAME", "runa")
        .current_dir(&project_dir)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("output type 'advance' conflicts with reserved session driver verb"),
        "stderr: {stderr}"
    );
}

#[tokio::test]
async fn session_surface_is_caller_agnostic_for_tools_readiness_and_context() {
    async fn snapshot(
        project_dir: &Path,
        caller_kind: &str,
    ) -> (Vec<String>, serde_json::Value, serde_json::Value) {
        let service = ()
            .serve(
                TokioChildProcess::new(Command::new(env!("CARGO_BIN_EXE_runa-mcp")).configure(
                    |cmd| {
                        cmd.arg("--work-unit")
                            .arg("work-unit-166")
                            .env_remove("GROUNDWORK_FORGE_TYPE")
                            .env_remove("GROUNDWORK_FORGE_TRACKER_ID")
                            .env("GROUNDWORK_FORGE_OWNER", "tesserine")
                            .env("GROUNDWORK_FORGE_NAME", "runa")
                            .env("RUNA_TEST_CALLER_KIND", caller_kind)
                            .current_dir(project_dir);
                    },
                ))
                .unwrap(),
            )
            .await
            .unwrap();

        let tools = service
            .list_all_tools()
            .await
            .unwrap()
            .into_iter()
            .map(|tool| tool.name.into_owned())
            .collect::<Vec<_>>();

        let readiness = service
            .call_tool(CallToolRequestParam {
                name: "readiness".into(),
                arguments: None,
            })
            .await
            .unwrap();
        let readiness: serde_json::Value = serde_json::from_str(text_content(&readiness)).unwrap();

        let context = service
            .call_tool(CallToolRequestParam {
                name: "next-protocol-context".into(),
                arguments: None,
            })
            .await
            .unwrap();
        let context: serde_json::Value = serde_json::from_str(text_content(&context)).unwrap();

        service.cancel().await.unwrap();
        (tools, readiness, context)
    }

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        scoped_work_unit_manifest_toml(),
        &scoped_work_unit_schemas(),
        &["take"],
    );
    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("work-unit")).unwrap();
    fs::write(
        workspace.join("work-unit/work-unit-166.json"),
        github_work_unit_json(166),
    )
    .unwrap();

    let interactive = snapshot(&project_dir, "interactive-driver").await;
    let autonomous = snapshot(&project_dir, "autonomous-orchestrator").await;

    assert_eq!(interactive.0, autonomous.0, "advertised tools differ");
    assert_eq!(interactive.1, autonomous.1, "readiness differs");
    assert_eq!(interactive.2, autonomous.2, "delivered context differs");
}

#[tokio::test]
async fn mcp_scans_workspace_before_rejecting_noncanonical_work_unit() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        scoped_work_unit_manifest_toml(),
        &scoped_work_unit_schemas(),
        &["take"],
    );
    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("work-unit")).unwrap();
    fs::write(
        workspace.join("work-unit/work-unit-163-scope.json"),
        r#"{"title":"Scope","description":"Enforce canonical scope","acceptance_criteria":["Reject aliases"]}"#,
    )
    .unwrap();

    let service = ()
        .serve(
            TokioChildProcess::new(
                Command::new(env!("CARGO_BIN_EXE_runa-mcp")).configure(|cmd| {
                    cmd.arg("--protocol")
                        .arg("take")
                        .arg("--work-unit")
                        .arg("163")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await;

    assert!(service.is_err());
}

#[tokio::test]
async fn mcp_accepts_exact_tracker_backed_work_unit_without_slug() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        scoped_work_unit_manifest_toml(),
        &scoped_work_unit_schemas(),
        &["take"],
    );
    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("work-unit")).unwrap();
    fs::write(
        workspace.join("work-unit/work-unit-163.json"),
        github_work_unit_json(163),
    )
    .unwrap();

    let service = ()
        .serve(
            TokioChildProcess::new(
                Command::new(env!("CARGO_BIN_EXE_runa-mcp")).configure(|cmd| {
                    cmd.arg("--protocol")
                        .arg("take")
                        .arg("--work-unit")
                        .arg("work-unit-163")
                        .env_remove("GROUNDWORK_FORGE_TYPE")
                        .env_remove("GROUNDWORK_FORGE_TRACKER_ID")
                        .env("GROUNDWORK_FORGE_OWNER", "tesserine")
                        .env("GROUNDWORK_FORGE_NAME", "runa")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let tools = service.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name.as_ref(), "claim");

    service.cancel().await.unwrap();
}

#[tokio::test]
async fn mcp_rejects_exact_tracker_backed_work_unit_with_number_disagreement() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        scoped_work_unit_manifest_toml(),
        &scoped_work_unit_schemas(),
        &["take"],
    );
    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("work-unit")).unwrap();
    fs::write(
        workspace.join("work-unit/work-unit-163.json"),
        github_work_unit_json(164),
    )
    .unwrap();

    let service = ()
        .serve(
            TokioChildProcess::new(
                Command::new(env!("CARGO_BIN_EXE_runa-mcp")).configure(|cmd| {
                    cmd.arg("--protocol")
                        .arg("take")
                        .arg("--work-unit")
                        .arg("work-unit-163")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await;

    assert!(service.is_err());
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
    assert!(!artifact.contains("instance_id"), "{artifact}");
    assert!(artifact.contains("\"work_unit\": \"wu-1\""), "{artifact}");

    service.cancel().await.unwrap();
}

#[tokio::test]
async fn tool_calls_append_transcript_events_when_enabled() {
    let dir = setup_project();
    let project_dir = dir.path().join("project");
    let transcript_dir = dir.path().join("transcript");

    let service = ()
        .serve(
            TokioChildProcess::new(
                Command::new(env!("CARGO_BIN_EXE_runa-mcp")).configure(|cmd| {
                    cmd.arg("--protocol")
                        .arg("implement")
                        .arg("--work-unit")
                        .arg("wu-1")
                        .env("RUNA_TRANSCRIPT_DIR", &transcript_dir)
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();

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

    let events = fs::read_to_string(transcript_dir.join("events.jsonl"))
        .expect("tool transcript events should be written");
    assert!(events.contains("\"source\":\"runa-mcp\""));
    assert!(events.contains("\"kind\":\"tool_call\""));
    assert!(events.contains("\"kind\":\"tool_result\""));
    assert!(events.contains("\"tool_name\":\"implementation\""));

    service.cancel().await.unwrap();
}

#[tokio::test]
async fn scoped_protocol_injects_required_work_unit_without_declared_property() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "implementation"

[[protocols]]
name = "implement"
produces = ["implementation"]
scoped = true
trigger = { type = "on_change", name = "implementation" }
"#,
        &[(
            "implementation",
            r#"{"type":"object","required":["title","work_unit"]}"#,
        )],
        &["implement"],
    );
    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

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
    assert!(!artifact.contains("instance_id"), "{artifact}");
    assert!(artifact.contains("\"work_unit\": \"wu-1\""), "{artifact}");

    service.cancel().await.unwrap();
}

#[tokio::test]
async fn scoped_protocol_injects_optional_work_unit_declared_in_properties() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "implementation"

[[protocols]]
name = "implement"
produces = ["implementation"]
scoped = true
trigger = { type = "on_change", name = "implementation" }
"#,
        &[(
            "implementation",
            r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"},"work_unit":{"type":"string"}}}"#,
        )],
        &["implement"],
    );
    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

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
    let tool_properties = tools[0]
        .input_schema
        .get("properties")
        .and_then(|value| value.as_object())
        .expect("tool schema should expose object properties");
    assert!(!tool_properties.contains_key("work_unit"));

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
