use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use rmcp::model::{CallToolRequestParam, CallToolResult};
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

fn scoped_two_step_manifest_toml() -> &'static str {
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

fn scoped_two_step_schemas() -> Vec<(&'static str, &'static str)> {
    let mut schemas = scoped_work_unit_schemas();
    schemas.push((
        "implementation",
        r#"{"type":"object","required":["work_unit","summary"],"properties":{"work_unit":{"type":"string"},"summary":{"type":"string"}}}"#,
    ));
    schemas
}

fn github_work_unit_json(number: u64) -> String {
    format!(
        r#"{{"title":"Scope","description":"Enforce canonical scope","acceptance_criteria":["Reject aliases"],"handle":{{"forge_tag":"github","url":"https://github.com/tesserine/runa/issues/{number}","number":{number}}}}}"#
    )
}

fn session_command(project_dir: &Path, caller: &str) -> TokioChildProcess {
    TokioChildProcess::new(
        Command::new(env!("CARGO_BIN_EXE_runa-mcp")).configure(|cmd| {
            cmd.arg("--session")
                .arg("--work-unit")
                .arg("work-unit-166")
                .env_remove("GROUNDWORK_FORGE_TYPE")
                .env_remove("GROUNDWORK_FORGE_TRACKER_ID")
                .env("GROUNDWORK_FORGE_OWNER", "tesserine")
                .env("GROUNDWORK_FORGE_NAME", "runa")
                .env("RUNA_TEST_CALLER", caller)
                .current_dir(project_dir);
        }),
    )
    .unwrap()
}

fn write_work_unit(project_dir: &Path) {
    let workspace = project_dir.join(".runa/workspace");
    fs::create_dir_all(workspace.join("work-unit")).unwrap();
    fs::write(
        workspace.join("work-unit/work-unit-166.json"),
        github_work_unit_json(166),
    )
    .unwrap();
}

fn tool_result_json(result: CallToolResult) -> serde_json::Value {
    let value = serde_json::to_value(result).unwrap();
    let text = value["content"][0]["text"]
        .as_str()
        .expect("tool result should contain text JSON");
    serde_json::from_str(text).unwrap()
}

fn tool_names(tools: &[rmcp::model::Tool]) -> Vec<String> {
    tools
        .iter()
        .map(|tool| tool.name.as_ref().to_string())
        .collect()
}

fn expected_take_prompt(project_dir: &Path) -> String {
    let mut loaded = libagent::project::load(project_dir, None).unwrap();
    libagent::scan(&loaded.workspace_dir, &mut loaded.store).unwrap();
    let protocol = loaded
        .manifest
        .protocols
        .iter()
        .find(|protocol| protocol.name == "take")
        .unwrap();
    let context = libagent::context::build_context(protocol, &loaded.store, Some("work-unit-166"));
    libagent::context::render_context_prompt(&context)
}

fn expected_readiness(project_dir: &Path) -> serde_json::Value {
    let mut loaded = libagent::project::load(project_dir, None).unwrap();
    let scan_result = libagent::scan(&loaded.workspace_dir, &mut loaded.store).unwrap();
    let state = libagent::evaluate_execution_state(
        &loaded,
        project_dir,
        &scan_result,
        libagent::EvaluationScope::Scoped("work-unit-166"),
    );
    serde_json::json!({
        "methodology": loaded.manifest.name,
        "scan_warnings": state.scan_findings.warnings,
        "protocols": state.evaluated.json_protocols(),
    })
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
async fn session_mode_advertises_driver_tools_and_current_output_tool() {
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
    write_work_unit(&project_dir);

    let service = ().serve(session_command(&project_dir, "interactive")).await.unwrap();

    let tools = service.list_all_tools().await.unwrap();

    assert_eq!(
        tool_names(&tools),
        vec!["readiness", "next-protocol-context", "advance", "claim"]
    );

    service.cancel().await.unwrap();
}

#[tokio::test]
async fn next_protocol_context_returns_structured_context_and_verbatim_prompt() {
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
    write_work_unit(&project_dir);

    let service = ().serve(session_command(&project_dir, "interactive")).await.unwrap();

    let payload = tool_result_json(
        service
            .call_tool(CallToolRequestParam {
                name: "next-protocol-context".into(),
                arguments: Some(serde_json::Map::new()),
            })
            .await
            .unwrap(),
    );

    assert_eq!(payload["current_step"]["protocol"], "take");
    assert_eq!(payload["context"]["protocol"], "take");
    assert_eq!(payload["context"]["work_unit"], "work-unit-166");
    assert_eq!(payload["prompt"], expected_take_prompt(&project_dir));
    assert_eq!(payload["readiness"], expected_readiness(&project_dir));

    service.cancel().await.unwrap();
}

#[tokio::test]
async fn session_surface_is_caller_agnostic_for_tools_readiness_and_context() {
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
    write_work_unit(&project_dir);

    let interactive = ().serve(session_command(&project_dir, "interactive")).await.unwrap();
    let autonomous = ().serve(session_command(&project_dir, "autonomous")).await.unwrap();

    assert_eq!(
        tool_names(&interactive.list_all_tools().await.unwrap()),
        tool_names(&autonomous.list_all_tools().await.unwrap())
    );

    let interactive_readiness = tool_result_json(
        interactive
            .call_tool(CallToolRequestParam {
                name: "readiness".into(),
                arguments: Some(serde_json::Map::new()),
            })
            .await
            .unwrap(),
    );
    let autonomous_readiness = tool_result_json(
        autonomous
            .call_tool(CallToolRequestParam {
                name: "readiness".into(),
                arguments: Some(serde_json::Map::new()),
            })
            .await
            .unwrap(),
    );
    assert_eq!(interactive_readiness, autonomous_readiness);

    let interactive_context = tool_result_json(
        interactive
            .call_tool(CallToolRequestParam {
                name: "next-protocol-context".into(),
                arguments: Some(serde_json::Map::new()),
            })
            .await
            .unwrap(),
    );
    let autonomous_context = tool_result_json(
        autonomous
            .call_tool(CallToolRequestParam {
                name: "next-protocol-context".into(),
                arguments: Some(serde_json::Map::new()),
            })
            .await
            .unwrap(),
    );
    assert_eq!(interactive_context, autonomous_context);

    interactive.cancel().await.unwrap();
    autonomous.cancel().await.unwrap();
}

#[tokio::test]
async fn record_read_advance_records_execution_for_the_recorded_step() {
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
    write_work_unit(&project_dir);

    let service = ().serve(session_command(&project_dir, "interactive")).await.unwrap();

    service
        .call_tool(CallToolRequestParam {
            name: "claim".into(),
            arguments: serde_json::json!({
                "instance_id": "claim-1",
                "scope": "session surface"
            })
            .as_object()
            .cloned(),
        })
        .await
        .unwrap();

    let before_advance = tool_result_json(
        service
            .call_tool(CallToolRequestParam {
                name: "readiness".into(),
                arguments: Some(serde_json::Map::new()),
            })
            .await
            .unwrap(),
    );
    assert_eq!(before_advance["current_step"]["protocol"], "take");

    let advance = tool_result_json(
        service
            .call_tool(CallToolRequestParam {
                name: "advance".into(),
                arguments: Some(serde_json::Map::new()),
            })
            .await
            .unwrap(),
    );
    assert_eq!(advance["advanced_step"]["protocol"], "take");

    let execution_records =
        fs::read_to_string(project_dir.join(".runa/store/execution-records.json")).unwrap();
    assert!(execution_records.contains(r#""protocol": "take""#));
    assert!(execution_records.contains(r#""work_unit": "work-unit-166""#));

    service.cancel().await.unwrap();
}

#[tokio::test]
async fn advance_regenerates_output_tools_for_the_new_current_step() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        scoped_two_step_manifest_toml(),
        &scoped_two_step_schemas(),
        &["take", "implement"],
    );
    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);
    write_work_unit(&project_dir);

    let service = ().serve(session_command(&project_dir, "interactive")).await.unwrap();

    assert_eq!(
        tool_names(&service.list_all_tools().await.unwrap()),
        vec!["readiness", "next-protocol-context", "advance", "claim"]
    );

    service
        .call_tool(CallToolRequestParam {
            name: "claim".into(),
            arguments: serde_json::json!({
                "instance_id": "claim-1",
                "scope": "session surface"
            })
            .as_object()
            .cloned(),
        })
        .await
        .unwrap();
    service
        .call_tool(CallToolRequestParam {
            name: "advance".into(),
            arguments: Some(serde_json::Map::new()),
        })
        .await
        .unwrap();

    assert_eq!(
        tool_names(&service.list_all_tools().await.unwrap()),
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
async fn session_refuses_current_step_with_reserved_driver_tool_name() {
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
            scoped_work_unit_schemas()[0],
            (
                "advance",
                r#"{"type":"object","required":["work_unit","scope"],"properties":{"work_unit":{"type":"string"},"scope":{"type":"string"}}}"#,
            ),
        ],
        &["take"],
    );
    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);
    write_work_unit(&project_dir);

    let service = ().serve(session_command(&project_dir, "interactive")).await;

    assert!(service.is_err());
}

#[tokio::test]
async fn advance_refuses_next_step_with_unsupported_required_output_schema() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_methodology(
        dir.path(),
        r#"
name = "groundwork"

[[artifact_types]]
name = "work-unit"

[[artifact_types]]
name = "claim"

[[artifact_types]]
name = "audit-log"

[[protocols]]
name = "take"
requires = ["work-unit"]
produces = ["claim"]
scoped = true
trigger = { type = "on_artifact", name = "work-unit" }

[[protocols]]
name = "audit"
requires = ["claim"]
produces = ["audit-log"]
scoped = true
trigger = { type = "on_artifact", name = "claim" }
"#,
        &[
            scoped_work_unit_schemas()[0],
            scoped_work_unit_schemas()[1],
            ("audit-log", r#"{"type":"array","items":{"type":"string"}}"#),
        ],
        &["take", "audit"],
    );
    let project_dir = dir.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    init_project(&project_dir, &manifest_path);
    write_work_unit(&project_dir);

    let service = ().serve(session_command(&project_dir, "interactive")).await.unwrap();
    service
        .call_tool(CallToolRequestParam {
            name: "claim".into(),
            arguments: serde_json::json!({
                "instance_id": "claim-1",
                "scope": "session surface"
            })
            .as_object()
            .cloned(),
        })
        .await
        .unwrap();

    let result = service
        .call_tool(CallToolRequestParam {
            name: "advance".into(),
            arguments: Some(serde_json::Map::new()),
        })
        .await;

    assert!(result.is_err());

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
