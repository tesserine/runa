use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::sync::Arc;
use std::time::Duration;

use rmcp::ClientHandler;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::ServiceExt;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use tokio::process::Command;
use tokio::sync::Notify;

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

fn forge_collision_manifest_toml() -> &'static str {
    r#"
name = "groundwork"

[[artifact_types]]
name = "read-ticket"

[[protocols]]
name = "take"
produces = ["read-ticket"]
trigger = { type = "on_change", name = "read-ticket" }
"#
}

fn forge_collision_schemas() -> Vec<(&'static str, &'static str)> {
    vec![(
        "read-ticket",
        r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
    )]
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

fn scoped_work_unit_with_unsupported_claim_schemas() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "work-unit",
            r#"{"type":"object","required":["title","description","acceptance_criteria"],"properties":{"title":{"type":"string"},"description":{"type":"string"},"acceptance_criteria":{"type":"array","items":{"type":"string"}},"handle":{"type":"object"}}}"#,
        ),
        ("claim", r#"{"type":"array","items":{"type":"string"}}"#),
    ]
}

fn two_step_session_manifest_toml() -> &'static str {
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

fn two_step_session_schemas() -> Vec<(&'static str, &'static str)> {
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
            r#"{"type":"object","required":["work_unit","summary"],"properties":{"work_unit":{"type":"string"},"summary":{"type":"string"}}}"#,
        ),
    ]
}

fn two_step_with_unsupported_next_output_schemas() -> Vec<(&'static str, &'static str)> {
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
            r#"{"type":"array","items":{"type":"string"}}"#,
        ),
    ]
}

struct ToolListChangeClient {
    notify: Arc<Notify>,
}

impl ClientHandler for ToolListChangeClient {
    async fn on_tool_list_changed(
        &self,
        _context: rmcp::service::NotificationContext<rmcp::RoleClient>,
    ) {
        self.notify.notify_one();
    }
}

fn github_work_unit_json(number: u64) -> String {
    github_work_unit_json_with_title(number, "Scope")
}

fn github_work_unit_json_with_title(number: u64, title: &str) -> String {
    format!(
        r#"{{"title":"{title}","description":"Enforce canonical scope","acceptance_criteria":["Reject aliases"],"handle":{{"forge_tag":"github","url":"https://github.com/tesserine/runa/issues/{number}","number":{number}}}}}"#
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

fn append_github_forge_config(project_dir: &Path, owner: &str, name: &str) {
    let config_path = project_dir.join(".runa/config.toml");
    let existing = fs::read_to_string(&config_path).unwrap();
    fs::write(
        config_path,
        format!("{existing}\n[forge]\ntype = \"github\"\nowner = \"{owner}\"\nname = \"{name}\"\n"),
    )
    .unwrap();
}

fn append_github_forge_config_with_api(
    project_dir: &Path,
    owner: &str,
    name: &str,
    api_base: &str,
) {
    let config_path = project_dir.join(".runa/config.toml");
    let existing = fs::read_to_string(&config_path).unwrap();
    fs::write(
        config_path,
        format!(
            "{existing}\n[forge]\ntype = \"github\"\nowner = \"{owner}\"\nname = \"{name}\"\napi_base = \"{api_base}\"\n",
        ),
    )
    .unwrap();
}

fn append_transcript_config(project_dir: &Path, transcript_dir: &Path) {
    let config_path = project_dir.join(".runa/config.toml");
    let existing = fs::read_to_string(&config_path).unwrap();
    fs::write(
        config_path,
        format!(
            "{existing}\n[transcript]\ndir = {:?}\n",
            transcript_dir.display().to_string()
        ),
    )
    .unwrap();
}

fn read_transcript_events(root: &Path) -> String {
    let mut events = String::new();
    collect_transcript_events(root, &mut events);
    events
}

fn collect_transcript_events(path: &Path, events: &mut String) {
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_transcript_events(&path, events);
        } else if path.file_name().and_then(|name| name.to_str()) == Some("events.jsonl") {
            events.push_str(&fs::read_to_string(path).unwrap());
        }
    }
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

fn tool_result_text(result: &CallToolResult) -> String {
    result.content[0]
        .as_text()
        .expect("tool result should be text")
        .text
        .clone()
}

fn session_call(name: &str) -> CallToolRequestParams {
    CallToolRequestParams::new(name.to_string())
}

fn tool_call(name: &str, arguments: serde_json::Value) -> CallToolRequestParams {
    CallToolRequestParams::new(name.to_string()).with_arguments(
        arguments
            .as_object()
            .expect("tool arguments must be an object")
            .clone(),
    )
}

fn canonical_forge_tool_names() -> Vec<&'static str> {
    vec![
        "apply-approved-change",
        "claim-work-unit",
        "close-out",
        "create-ticket",
        "deliver-change-proposal",
        "read-ticket",
        "record-progress",
        "reflect-disposition",
    ]
}

#[tokio::test]
async fn call_tool_rejects_forge_artifact_name_collision_before_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        forge_collision_manifest_toml(),
        &forge_collision_schemas(),
        &["take"],
    );
    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);
    append_github_forge_config(&project_dir, "tesserine", "runa");

    let service = ()
        .serve(
            TokioChildProcess::new(
                Command::new(env!("CARGO_BIN_EXE_runa-mcp")).configure(|cmd| {
                    cmd.arg("--protocol")
                        .arg("take")
                        .env_remove("RUNA_FORGE_TYPE")
                        .env_remove("RUNA_FORGE_OWNER")
                        .env_remove("RUNA_FORGE_NAME")
                        .env_remove("RUNA_FORGE_TRACKER_ID")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let result = service
        .call_tool(tool_call(
            "read-ticket",
            serde_json::json!({
                "instance_id": "artifact-1",
                "title": "must not dispatch to forge"
            }),
        ))
        .await
        .unwrap();
    let text = tool_result_text(&result);
    assert!(
        text.contains("tool name collision") && text.contains("read-ticket"),
        "colliding tool name should be rejected before forge dispatch: {text}"
    );
    assert!(
        !project_dir
            .join(".runa/workspace/read-ticket/artifact-1.json")
            .exists(),
        "colliding call must not write an artifact"
    );

    service.cancel().await.unwrap();
}

fn assert_no_execution_record_for(project_dir: &Path, protocol: &str) {
    let execution_record_path = project_dir.join(".runa/store/execution-records.json");
    if execution_record_path.is_file() {
        let execution_records = fs::read_to_string(&execution_record_path).unwrap();
        assert!(
            !execution_records.contains(&format!(r#""protocol": "{protocol}""#)),
            "advance must not record execution for {protocol}: {execution_records}"
        );
    }
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
async fn session_mode_advertises_driver_verbs_and_current_output_tools() {
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
                    cmd.arg("--session")
                        .arg("--work-unit")
                        .arg("work-unit-166")
                        .env_remove("RUNA_FORGE_TYPE")
                        .env_remove("RUNA_FORGE_TRACKER_ID")
                        .env("RUNA_FORGE_OWNER", "tesserine")
                        .env("RUNA_FORGE_NAME", "runa")
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
async fn session_mode_is_caller_agnostic_for_tools_readiness_and_context() {
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

    let service_a = ()
        .serve(
            TokioChildProcess::new(
                Command::new(env!("CARGO_BIN_EXE_runa-mcp")).configure(|cmd| {
                    cmd.arg("--session")
                        .arg("--work-unit")
                        .arg("work-unit-166")
                        .env("RUNA_CALLER_KIND", "interactive")
                        .env_remove("RUNA_FORGE_TYPE")
                        .env_remove("RUNA_FORGE_TRACKER_ID")
                        .env("RUNA_FORGE_OWNER", "tesserine")
                        .env("RUNA_FORGE_NAME", "runa")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let service_b = ()
        .serve(
            TokioChildProcess::new(
                Command::new(env!("CARGO_BIN_EXE_runa-mcp")).configure(|cmd| {
                    cmd.arg("--session")
                        .arg("--work-unit")
                        .arg("work-unit-166")
                        .env("RUNA_CALLER_KIND", "autonomous")
                        .env_remove("RUNA_FORGE_TYPE")
                        .env_remove("RUNA_FORGE_TRACKER_ID")
                        .env("RUNA_FORGE_OWNER", "tesserine")
                        .env("RUNA_FORGE_NAME", "runa")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let tools_a = service_a
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|tool| tool.name.into_owned())
        .collect::<Vec<_>>();
    let tools_b = service_b
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|tool| tool.name.into_owned())
        .collect::<Vec<_>>();
    assert_eq!(tools_a, tools_b);

    let readiness_a = service_a
        .call_tool(session_call("readiness"))
        .await
        .unwrap();
    let readiness_b = service_b
        .call_tool(session_call("readiness"))
        .await
        .unwrap();
    assert_eq!(
        tool_result_text(&readiness_a),
        tool_result_text(&readiness_b)
    );

    let context_a = service_a
        .call_tool(session_call("next-protocol-context"))
        .await
        .unwrap();
    let context_b = service_b
        .call_tool(session_call("next-protocol-context"))
        .await
        .unwrap();
    assert_eq!(tool_result_text(&context_a), tool_result_text(&context_b));

    service_a.cancel().await.unwrap();
    service_b.cancel().await.unwrap();
}

#[tokio::test]
async fn session_record_read_advance_records_execution_for_producing_step() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        two_step_session_manifest_toml(),
        &two_step_session_schemas(),
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
                    cmd.arg("--session")
                        .arg("--work-unit")
                        .arg("work-unit-166")
                        .env_remove("RUNA_FORGE_TYPE")
                        .env_remove("RUNA_FORGE_TRACKER_ID")
                        .env("RUNA_FORGE_OWNER", "tesserine")
                        .env("RUNA_FORGE_NAME", "runa")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    service
        .call_tool(tool_call(
            "claim",
            serde_json::json!({
                "instance_id": "claim-1",
                "scope": "claim this work"
            }),
        ))
        .await
        .unwrap();

    service.call_tool(session_call("readiness")).await.unwrap();
    service.call_tool(session_call("advance")).await.unwrap();

    let execution_records =
        fs::read_to_string(project_dir.join(".runa/store/execution-records.json")).unwrap();
    assert!(
        execution_records.contains(r#""protocol": "take""#),
        "{execution_records}"
    );

    let tool_names = service
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|tool| tool.name.into_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        tool_names,
        vec![
            "readiness",
            "next-protocol-context",
            "advance",
            "implementation"
        ]
    );

    service.cancel().await.unwrap();
}

#[tokio::test]
async fn session_advance_records_context_time_input_provenance() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        two_step_session_manifest_toml(),
        &two_step_session_schemas(),
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
                    cmd.arg("--session")
                        .arg("--work-unit")
                        .arg("work-unit-166")
                        .env_remove("RUNA_FORGE_TYPE")
                        .env_remove("RUNA_FORGE_TRACKER_ID")
                        .env("RUNA_FORGE_OWNER", "tesserine")
                        .env("RUNA_FORGE_NAME", "runa")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let context = service
        .call_tool(session_call("next-protocol-context"))
        .await
        .unwrap();
    let context: serde_json::Value = serde_json::from_str(&tool_result_text(&context)).unwrap();
    let context_hash = context["context"]["inputs"][0]["content_hash"]
        .as_str()
        .expect("context input should carry content hash")
        .to_string();

    fs::write(
        workspace.join("work-unit/work-unit-166.json"),
        github_work_unit_json_with_title(166, "Scope revised"),
    )
    .unwrap();

    service
        .call_tool(tool_call(
            "claim",
            serde_json::json!({
                "instance_id": "claim-1",
                "scope": "claim this work"
            }),
        ))
        .await
        .unwrap();
    service.call_tool(session_call("advance")).await.unwrap();

    let execution_records: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(project_dir.join(".runa/store/execution-records.json")).unwrap(),
    )
    .unwrap();
    let recorded_hash = execution_records["records"][0]["inputs"]["artifact_types"]["work-unit"][0]
        ["content_hash"]
        .as_str()
        .expect("execution record should include work-unit input hash");
    assert_eq!(recorded_hash, context_hash);

    let current_state: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(project_dir.join(".runa/store/work-unit/work-unit-166.json")).unwrap(),
    )
    .unwrap();
    let current_hash = current_state["content_hash"]
        .as_str()
        .expect("store state should include current content hash");
    assert_ne!(current_hash, context_hash);

    service.cancel().await.unwrap();
}

#[tokio::test]
async fn session_advance_reopens_current_step_when_context_input_changes() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        two_step_session_manifest_toml(),
        &two_step_session_schemas(),
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
                    cmd.arg("--session")
                        .arg("--work-unit")
                        .arg("work-unit-166")
                        .env_remove("RUNA_FORGE_TYPE")
                        .env_remove("RUNA_FORGE_TRACKER_ID")
                        .env("RUNA_FORGE_OWNER", "tesserine")
                        .env("RUNA_FORGE_NAME", "runa")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    service
        .call_tool(session_call("next-protocol-context"))
        .await
        .unwrap();
    fs::write(
        workspace.join("work-unit/work-unit-166.json"),
        github_work_unit_json_with_title(166, "Scope revised"),
    )
    .unwrap();
    service
        .call_tool(tool_call(
            "claim",
            serde_json::json!({
                "instance_id": "claim-1",
                "scope": "claim this work"
            }),
        ))
        .await
        .unwrap();

    let advance = service.call_tool(session_call("advance")).await.unwrap();
    let advance: serde_json::Value = serde_json::from_str(&tool_result_text(&advance)).unwrap();
    assert_eq!(advance["completed_step"]["protocol"], "take");
    assert_eq!(advance["next_step"]["protocol"], "take");

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

    let context = service
        .call_tool(session_call("next-protocol-context"))
        .await
        .unwrap();
    let context: serde_json::Value = serde_json::from_str(&tool_result_text(&context)).unwrap();
    assert_eq!(context["context"]["protocol"], "take");

    service.cancel().await.unwrap();
}

#[tokio::test]
async fn session_advance_reopens_current_step_when_readiness_consumes_context_input_change() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        two_step_session_manifest_toml(),
        &two_step_session_schemas(),
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
                    cmd.arg("--session")
                        .arg("--work-unit")
                        .arg("work-unit-166")
                        .env_remove("RUNA_FORGE_TYPE")
                        .env_remove("RUNA_FORGE_TRACKER_ID")
                        .env("RUNA_FORGE_OWNER", "tesserine")
                        .env("RUNA_FORGE_NAME", "runa")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    service
        .call_tool(session_call("next-protocol-context"))
        .await
        .unwrap();
    fs::write(
        workspace.join("work-unit/work-unit-166.json"),
        github_work_unit_json_with_title(166, "Scope revised"),
    )
    .unwrap();
    service.call_tool(session_call("readiness")).await.unwrap();
    service
        .call_tool(tool_call(
            "claim",
            serde_json::json!({
                "instance_id": "claim-1",
                "scope": "claim this work"
            }),
        ))
        .await
        .unwrap();

    let advance = service.call_tool(session_call("advance")).await.unwrap();
    let advance: serde_json::Value = serde_json::from_str(&tool_result_text(&advance)).unwrap();
    assert_eq!(advance["completed_step"]["protocol"], "take");
    assert_eq!(advance["next_step"]["protocol"], "take");

    service.cancel().await.unwrap();
}

#[tokio::test]
async fn session_advance_emits_tool_list_changed_when_current_step_changes() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        two_step_session_manifest_toml(),
        &two_step_session_schemas(),
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

    let tool_list_changed = Arc::new(Notify::new());
    let service = ToolListChangeClient {
        notify: tool_list_changed.clone(),
    }
    .serve(
        TokioChildProcess::new(
            Command::new(env!("CARGO_BIN_EXE_runa-mcp")).configure(|cmd| {
                cmd.arg("--session")
                    .arg("--work-unit")
                    .arg("work-unit-166")
                    .env_remove("RUNA_FORGE_TYPE")
                    .env_remove("RUNA_FORGE_TRACKER_ID")
                    .env("RUNA_FORGE_OWNER", "tesserine")
                    .env("RUNA_FORGE_NAME", "runa")
                    .current_dir(&project_dir);
            }),
        )
        .unwrap(),
    )
    .await
    .unwrap();

    service
        .call_tool(tool_call(
            "claim",
            serde_json::json!({
                "instance_id": "claim-1",
                "scope": "claim this work"
            }),
        ))
        .await
        .unwrap();

    service.call_tool(session_call("advance")).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), tool_list_changed.notified())
        .await
        .expect("advance should emit notifications/tools/list_changed");

    service.cancel().await.unwrap();
}

#[tokio::test]
async fn session_advance_reconciles_deleted_output_before_recording_execution() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        two_step_session_manifest_toml(),
        &two_step_session_schemas(),
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
                    cmd.arg("--session")
                        .arg("--work-unit")
                        .arg("work-unit-166")
                        .env_remove("RUNA_FORGE_TYPE")
                        .env_remove("RUNA_FORGE_TRACKER_ID")
                        .env("RUNA_FORGE_OWNER", "tesserine")
                        .env("RUNA_FORGE_NAME", "runa")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    service
        .call_tool(tool_call(
            "claim",
            serde_json::json!({
                "instance_id": "claim-1",
                "scope": "claim this work"
            }),
        ))
        .await
        .unwrap();
    fs::remove_file(workspace.join("claim/claim-1.json")).unwrap();

    let advance = service.call_tool(session_call("advance")).await;
    assert!(
        advance.is_err(),
        "advance unexpectedly succeeded: {advance:?}"
    );
    let execution_record_path = project_dir.join(".runa/store/execution-records.json");
    if execution_record_path.exists() {
        let execution_records = fs::read_to_string(&execution_record_path).unwrap();
        assert!(
            !execution_records.contains(r#""protocol": "take""#),
            "advance must not record execution from stale output state: {execution_records}"
        );
    }

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
async fn session_advance_rejects_deleted_required_input_before_downstream_selection() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        two_step_session_manifest_toml(),
        &two_step_session_schemas(),
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
                    cmd.arg("--session")
                        .arg("--work-unit")
                        .arg("work-unit-166")
                        .env_remove("RUNA_FORGE_TYPE")
                        .env_remove("RUNA_FORGE_TRACKER_ID")
                        .env("RUNA_FORGE_OWNER", "tesserine")
                        .env("RUNA_FORGE_NAME", "runa")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    service
        .call_tool(tool_call(
            "claim",
            serde_json::json!({
                "instance_id": "claim-1",
                "scope": "claim this work"
            }),
        ))
        .await
        .unwrap();
    fs::remove_file(workspace.join("work-unit/work-unit-166.json")).unwrap();

    let advance = service.call_tool(session_call("advance")).await;
    assert!(
        advance.is_err(),
        "advance unexpectedly succeeded after required input deletion: {advance:?}"
    );
    assert_no_execution_record_for(&project_dir, "take");

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
async fn session_readiness_selects_later_ready_step_and_advertises_tools() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        two_step_session_manifest_toml(),
        &two_step_session_schemas(),
        &["take", "implement"],
    );
    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");

    let service = ()
        .serve(
            TokioChildProcess::new(
                Command::new(env!("CARGO_BIN_EXE_runa-mcp")).configure(|cmd| {
                    cmd.arg("--session")
                        .arg("--work-unit")
                        .arg("work-unit-166")
                        .env_remove("RUNA_FORGE_TYPE")
                        .env_remove("RUNA_FORGE_TRACKER_ID")
                        .env("RUNA_FORGE_OWNER", "tesserine")
                        .env("RUNA_FORGE_NAME", "runa")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let initial_tool_names = service
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|tool| tool.name.into_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        initial_tool_names,
        vec!["readiness", "next-protocol-context", "advance"]
    );

    fs::create_dir_all(workspace.join("work-unit")).unwrap();
    fs::write(
        workspace.join("work-unit/work-unit-166.json"),
        github_work_unit_json(166),
    )
    .unwrap();

    let readiness = service.call_tool(session_call("readiness")).await.unwrap();
    let readiness: serde_json::Value = serde_json::from_str(&tool_result_text(&readiness)).unwrap();
    assert_eq!(readiness["current_step"]["protocol"], "take");

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
async fn session_readiness_rejects_newly_ready_step_with_unservable_required_output() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        scoped_work_unit_manifest_toml(),
        &scoped_work_unit_with_unsupported_claim_schemas(),
        &["take"],
    );
    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");

    let service = ()
        .serve(
            TokioChildProcess::new(
                Command::new(env!("CARGO_BIN_EXE_runa-mcp")).configure(|cmd| {
                    cmd.arg("--session")
                        .arg("--work-unit")
                        .arg("work-unit-166")
                        .env_remove("RUNA_FORGE_TYPE")
                        .env_remove("RUNA_FORGE_TRACKER_ID")
                        .env("RUNA_FORGE_OWNER", "tesserine")
                        .env("RUNA_FORGE_NAME", "runa")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    fs::create_dir_all(workspace.join("work-unit")).unwrap();
    fs::write(
        workspace.join("work-unit/work-unit-166.json"),
        github_work_unit_json(166),
    )
    .unwrap();

    let readiness = service.call_tool(session_call("readiness")).await;
    assert!(
        readiness.is_err(),
        "readiness unexpectedly entered an unservable step: {readiness:?}"
    );

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

    let context = service
        .call_tool(session_call("next-protocol-context"))
        .await;
    assert!(
        context.is_err(),
        "next-protocol-context unexpectedly found a current step: {context:?}"
    );

    service.cancel().await.unwrap();
}

#[tokio::test]
async fn session_readiness_emits_tool_list_changed_when_current_step_becomes_ready() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        two_step_session_manifest_toml(),
        &two_step_session_schemas(),
        &["take", "implement"],
    );
    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let workspace = project_dir.join(".runa/workspace");
    let tool_list_changed = Arc::new(Notify::new());
    let service = ToolListChangeClient {
        notify: tool_list_changed.clone(),
    }
    .serve(
        TokioChildProcess::new(
            Command::new(env!("CARGO_BIN_EXE_runa-mcp")).configure(|cmd| {
                cmd.arg("--session")
                    .arg("--work-unit")
                    .arg("work-unit-166")
                    .env_remove("RUNA_FORGE_TYPE")
                    .env_remove("RUNA_FORGE_TRACKER_ID")
                    .env("RUNA_FORGE_OWNER", "tesserine")
                    .env("RUNA_FORGE_NAME", "runa")
                    .current_dir(&project_dir);
            }),
        )
        .unwrap(),
    )
    .await
    .unwrap();

    fs::create_dir_all(workspace.join("work-unit")).unwrap();
    fs::write(
        workspace.join("work-unit/work-unit-166.json"),
        github_work_unit_json(166),
    )
    .unwrap();

    service.call_tool(session_call("readiness")).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), tool_list_changed.notified())
        .await
        .expect("readiness should emit notifications/tools/list_changed");

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
async fn session_advance_error_preserves_current_step_when_next_step_is_unservable() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        two_step_session_manifest_toml(),
        &two_step_with_unsupported_next_output_schemas(),
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
                    cmd.arg("--session")
                        .arg("--work-unit")
                        .arg("work-unit-166")
                        .env_remove("RUNA_FORGE_TYPE")
                        .env_remove("RUNA_FORGE_TRACKER_ID")
                        .env("RUNA_FORGE_OWNER", "tesserine")
                        .env("RUNA_FORGE_NAME", "runa")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    service
        .call_tool(tool_call(
            "claim",
            serde_json::json!({
                "instance_id": "claim-1",
                "scope": "claim this work"
            }),
        ))
        .await
        .unwrap();

    let advance = service.call_tool(session_call("advance")).await;
    assert!(
        advance.is_err(),
        "advance unexpectedly succeeded: {advance:?}"
    );
    assert_no_execution_record_for(&project_dir, "take");

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

    let context = service
        .call_tool(session_call("next-protocol-context"))
        .await
        .unwrap();
    let context: serde_json::Value = serde_json::from_str(&tool_result_text(&context)).unwrap();
    assert_eq!(context["context"]["protocol"], "take");

    service.cancel().await.unwrap();
}

#[tokio::test]
async fn session_advance_persistence_error_preserves_current_step_and_no_record() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        two_step_session_manifest_toml(),
        &two_step_session_schemas(),
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
                    cmd.arg("--session")
                        .arg("--work-unit")
                        .arg("work-unit-166")
                        .env_remove("RUNA_FORGE_TYPE")
                        .env_remove("RUNA_FORGE_TRACKER_ID")
                        .env("RUNA_FORGE_OWNER", "tesserine")
                        .env("RUNA_FORGE_NAME", "runa")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    service
        .call_tool(tool_call(
            "claim",
            serde_json::json!({
                "instance_id": "claim-1",
                "scope": "claim this work"
            }),
        ))
        .await
        .unwrap();
    let execution_record_path = project_dir.join(".runa/store/execution-records.json");
    if execution_record_path.is_file() {
        fs::remove_file(&execution_record_path).unwrap();
    }
    fs::create_dir_all(&execution_record_path).unwrap();

    let advance = service.call_tool(session_call("advance")).await;
    assert!(
        advance.is_err(),
        "advance unexpectedly succeeded: {advance:?}"
    );
    assert_no_execution_record_for(&project_dir, "take");

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

    let context = service
        .call_tool(session_call("next-protocol-context"))
        .await
        .unwrap();
    let context: serde_json::Value = serde_json::from_str(&tool_result_text(&context)).unwrap();
    assert_eq!(context["context"]["protocol"], "take");

    service.cancel().await.unwrap();
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
async fn session_start_rejects_may_produce_only_reserved_driver_output() {
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
name = "audit"
requires = ["work-unit"]
may_produce = ["advance"]
scoped = true
trigger = { type = "on_artifact", name = "work-unit" }
"#,
        &[
            (
                "work-unit",
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
            ),
            (
                "advance",
                r#"{"type":"object","required":["summary"],"properties":{"summary":{"type":"string"}}}"#,
            ),
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
        r#"{"title":"Scope"}"#,
    )
    .unwrap();

    let service = ()
        .serve(
            TokioChildProcess::new(
                Command::new(env!("CARGO_BIN_EXE_runa-mcp")).configure(|cmd| {
                    cmd.arg("--session")
                        .arg("--work-unit")
                        .arg("work-unit-166")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await;

    if let Ok(service) = service {
        service.cancel().await.unwrap();
        panic!("session unexpectedly started with reserved may_produce output");
    }
}

#[tokio::test]
async fn fixed_protocol_mode_exposes_output_tool_named_advance() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "advance"

[[protocols]]
name = "legacy"
produces = ["advance"]
trigger = { type = "on_change", name = "advance" }
"#,
        &[(
            "advance",
            r#"{"type":"object","required":["summary"],"properties":{"summary":{"type":"string"}}}"#,
        )],
        &["legacy"],
    );
    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);

    let service = ()
        .serve(
            TokioChildProcess::new(
                Command::new(env!("CARGO_BIN_EXE_runa-mcp")).configure(|cmd| {
                    cmd.arg("--protocol")
                        .arg("legacy")
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
    assert_eq!(tool_names, vec!["advance"]);

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
                        .env_remove("RUNA_FORGE_TYPE")
                        .env_remove("RUNA_FORGE_TRACKER_ID")
                        .env("RUNA_FORGE_OWNER", "tesserine")
                        .env("RUNA_FORGE_NAME", "runa")
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
async fn mcp_accepts_tracker_backed_work_unit_with_forge_identity_only_in_config() {
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
    append_github_forge_config(&project_dir, "tesserine", "runa");

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
                        .env_remove("RUNA_FORGE_TYPE")
                        .env_remove("RUNA_FORGE_OWNER")
                        .env_remove("RUNA_FORGE_NAME")
                        .env_remove("RUNA_FORGE_TRACKER_ID")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let tools = service.list_all_tools().await.unwrap();
    let mut tool_names = tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();
    tool_names.sort_unstable();

    let mut expected_names = vec!["claim"];
    expected_names.extend(canonical_forge_tool_names());
    expected_names.sort_unstable();
    assert_eq!(tool_names, expected_names);

    let read_ticket = tools
        .iter()
        .find(|tool| tool.name.as_ref() == "read-ticket")
        .expect("read-ticket connector tool should be advertised");
    assert!(
        read_ticket.output_schema.is_some(),
        "forge connector tools should advertise output schemas"
    );

    service.cancel().await.unwrap();
}

#[tokio::test]
async fn mcp_forge_connector_uses_resolved_override_identity() {
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
    append_github_forge_config_with_api(
        &project_dir,
        "stale-owner",
        "stale-repo",
        "http://127.0.0.1:1",
    );

    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("work-unit")).unwrap();
    fs::write(
        workspace.join("work-unit/work-unit-163.json"),
        r#"{"title":"Scope","description":"Enforce canonical scope","acceptance_criteria":["Reject aliases"],"handle":{"forge_tag":"github","url":"https://github.com/override-owner/override-repo/issues/163","number":163}}"#,
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
                        .env("RUNA_FORGE_TYPE", "github")
                        .env("RUNA_FORGE_OWNER", "override-owner")
                        .env("RUNA_FORGE_NAME", "override-repo")
                        .env_remove("RUNA_FORGE_TRACKER_ID")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let tools = service.list_all_tools().await.unwrap();
    assert!(
        tools.iter().any(|tool| tool.name.as_ref() == "read-ticket"),
        "forge connector tools should be advertised"
    );

    let result = service
        .call_tool(tool_call(
            "read-ticket",
            serde_json::json!({ "reference": "stale-owner/stale-repo#203" }),
        ))
        .await
        .unwrap();
    let text = tool_result_text(&result);
    assert!(
        text.contains("foreign scope") && text.contains("override-owner/override-repo"),
        "stale file-config identity should not be accepted: {text}"
    );

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
        .call_tool(tool_call(
            "implementation",
            serde_json::json!({
                "instance_id": "impl-1",
                "title": "ship it"
            }),
        ))
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
        .call_tool(tool_call(
            "implementation",
            serde_json::json!({
                "instance_id": "impl-1",
                "title": "ship it"
            }),
        ))
        .await
        .unwrap();

    let events = read_transcript_events(&transcript_dir);
    assert!(events.contains("\"source\":\"runa-mcp\""));
    assert!(events.contains("\"kind\":\"tool_call\""));
    assert!(events.contains("\"kind\":\"tool_result\""));
    assert!(events.contains("\"tool_name\":\"implementation\""));
    assert!(events.contains("\"schema_version\":2"));
    assert!(events.contains("\"work_unit\":\"wu-1\""));
    assert!(events.contains("\"run_id\":\"run-"));

    service.cancel().await.unwrap();
}

#[tokio::test]
async fn tool_calls_append_transcript_events_from_config_when_environment_is_unset() {
    let dir = setup_project();
    let project_dir = dir.path().join("project");
    let transcript_dir = dir.path().join("configured-transcript");
    append_transcript_config(&project_dir, &transcript_dir);

    let service = ()
        .serve(
            TokioChildProcess::new(
                Command::new(env!("CARGO_BIN_EXE_runa-mcp")).configure(|cmd| {
                    cmd.arg("--protocol")
                        .arg("implement")
                        .arg("--work-unit")
                        .arg("wu-1")
                        .env_remove("RUNA_TRANSCRIPT_DIR")
                        .env_remove("RUNA_TRANSCRIPT_REDACT_ENV")
                        .env_remove("RUNA_FORGE_TYPE")
                        .env_remove("RUNA_FORGE_OWNER")
                        .env_remove("RUNA_FORGE_NAME")
                        .env_remove("RUNA_FORGE_TRACKER_ID")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    service
        .call_tool(tool_call(
            "implementation",
            serde_json::json!({
                "instance_id": "impl-1",
                "title": "ship it"
            }),
        ))
        .await
        .unwrap();

    let events = read_transcript_events(&transcript_dir);
    assert!(events.contains("\"source\":\"runa-mcp\""));
    assert!(events.contains("\"kind\":\"tool_call\""));
    assert!(events.contains("\"kind\":\"tool_result\""));
    assert!(events.contains("\"tool_name\":\"implementation\""));
    assert!(events.contains("\"schema_version\":2"));
    assert!(events.contains("\"deployment\":\"project:sha256:"));
    assert!(events.contains("\"run_id\":\"run-"));

    service.cancel().await.unwrap();
}

#[tokio::test]
async fn forge_tool_calls_append_transcript_events_when_enabled() {
    let dir = setup_project();
    let project_dir = dir.path().join("project");
    let transcript_dir = dir.path().join("transcript");
    append_github_forge_config(&project_dir, "tesserine", "runa");

    let service = ()
        .serve(
            TokioChildProcess::new(
                Command::new(env!("CARGO_BIN_EXE_runa-mcp")).configure(|cmd| {
                    cmd.arg("--protocol")
                        .arg("implement")
                        .arg("--work-unit")
                        .arg("wu-1")
                        .env("RUNA_TRANSCRIPT_DIR", &transcript_dir)
                        .env_remove("RUNA_FORGE_TYPE")
                        .env_remove("RUNA_FORGE_OWNER")
                        .env_remove("RUNA_FORGE_NAME")
                        .env_remove("RUNA_FORGE_TRACKER_ID")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    service
        .call_tool(tool_call(
            "read-ticket",
            serde_json::json!({ "reference": "tesserine/groundwork#203" }),
        ))
        .await
        .unwrap();

    let events = if transcript_dir.exists() {
        read_transcript_events(&transcript_dir)
    } else {
        String::new()
    };
    assert!(
        events.contains(r#""kind":"tool_call","protocol":"implement","work_unit":"wu-1","tool_name":"read-ticket""#),
        "missing forge tool_call transcript event: {events}"
    );
    assert!(
        events.contains(r#""kind":"tool_result","protocol":"implement","work_unit":"wu-1","tool_name":"read-ticket""#),
        "missing forge tool_result transcript event: {events}"
    );

    service.cancel().await.unwrap();
}

#[tokio::test]
async fn session_driver_calls_append_transcript_events_when_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        two_step_session_manifest_toml(),
        &two_step_session_schemas(),
        &["take", "implement"],
    );
    let project_dir = dir.path().join("project");
    let transcript_dir = dir.path().join("transcript");
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
                    cmd.arg("--session")
                        .arg("--work-unit")
                        .arg("work-unit-166")
                        .env("RUNA_TRANSCRIPT_DIR", &transcript_dir)
                        .env_remove("RUNA_FORGE_TYPE")
                        .env_remove("RUNA_FORGE_TRACKER_ID")
                        .env("RUNA_FORGE_OWNER", "tesserine")
                        .env("RUNA_FORGE_NAME", "runa")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    service.call_tool(session_call("readiness")).await.unwrap();
    service
        .call_tool(session_call("next-protocol-context"))
        .await
        .unwrap();
    service
        .call_tool(tool_call(
            "claim",
            serde_json::json!({
                "instance_id": "claim-1",
                "scope": "claim this work"
            }),
        ))
        .await
        .unwrap();
    service.call_tool(session_call("advance")).await.unwrap();

    let events = read_transcript_events(&transcript_dir);
    assert!(events.contains("\"deployment\":\"github:tesserine/runa\""));
    assert!(events.contains("\"run_id\":\"run-"));
    for tool_name in ["readiness", "next-protocol-context", "advance"] {
        assert!(
            events.contains(&format!(r#""kind":"tool_call","protocol":"take","work_unit":"work-unit-166","tool_name":"{tool_name}""#)),
            "missing driver tool_call for {tool_name}: {events}"
        );
        assert!(
            events.contains(&format!(r#""kind":"tool_result","protocol":"take","work_unit":"work-unit-166","tool_name":"{tool_name}""#)),
            "missing driver tool_result for {tool_name}: {events}"
        );
    }

    service.cancel().await.unwrap();
}

#[tokio::test]
async fn failed_session_driver_calls_append_transcript_result_when_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        two_step_session_manifest_toml(),
        &two_step_session_schemas(),
        &["take", "implement"],
    );
    let project_dir = dir.path().join("project");
    let transcript_dir = dir.path().join("transcript");
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
                    cmd.arg("--session")
                        .arg("--work-unit")
                        .arg("work-unit-166")
                        .env("RUNA_TRANSCRIPT_DIR", &transcript_dir)
                        .env_remove("RUNA_FORGE_TYPE")
                        .env_remove("RUNA_FORGE_TRACKER_ID")
                        .env("RUNA_FORGE_OWNER", "tesserine")
                        .env("RUNA_FORGE_NAME", "runa")
                        .current_dir(&project_dir);
                }),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let advance = service.call_tool(session_call("advance")).await;
    assert!(
        advance.is_err(),
        "advance unexpectedly succeeded: {advance:?}"
    );

    let events = read_transcript_events(&transcript_dir);
    assert!(
        events.contains(r#""kind":"tool_call","protocol":"take","work_unit":"work-unit-166","tool_name":"advance""#),
        "missing failed advance tool_call: {events}"
    );
    assert!(
        events.contains(r#""kind":"tool_result","protocol":"take","work_unit":"work-unit-166","tool_name":"advance""#),
        "missing failed advance tool_result: {events}"
    );
    assert!(
        events.contains("post-execution") && events.contains("claim"),
        "failed advance transcript should contain the error message: {events}"
    );

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
        .call_tool(tool_call(
            "implementation",
            serde_json::json!({
                "instance_id": "impl-1",
                "title": "ship it"
            }),
        ))
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
        .call_tool(tool_call(
            "implementation",
            serde_json::json!({
                "instance_id": "impl-1",
                "title": "ship it"
            }),
        ))
        .await
        .unwrap();

    let artifact =
        fs::read_to_string(project_dir.join(".runa/workspace/implementation/impl-1.json")).unwrap();
    assert!(artifact.contains("\"title\": \"ship it\""), "{artifact}");
    assert!(artifact.contains("\"work_unit\": \"wu-1\""), "{artifact}");

    service.cancel().await.unwrap();
}
