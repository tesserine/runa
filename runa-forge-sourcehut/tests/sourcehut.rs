use apollo_compiler::{ExecutableDocument, Schema};
use runa_forge_contract::Operation;
use runa_forge_sourcehut::{
    ProviderRequest, SourcehutConfig, SourcehutConnector, SourcehutHttpTransport,
    SourcehutRecordingTransport,
};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::thread;

const SOURCEHUT_TODO_SCHEMA: &str = include_str!("fixtures/todo.sr.ht.schema.graphqls");

fn config(api_base: &str) -> SourcehutConfig {
    SourcehutConfig {
        tracker_id: "4".to_string(),
        api_base: api_base.to_string(),
        git_remote: "ssh://git@git.sr.ht/~tesserine/runa".to_string(),
        credential_env: None,
        credential_command: None,
    }
}

fn handle(number: u64) -> serde_json::Value {
    json!({
        "id": format!("sourcehut:tracker:4:ticket:{number}"),
        "display": format!("sourcehut:4#{number}")
    })
}

fn change() -> serde_json::Value {
    json!({
        "id": "sourcehut:tracker:4:change:issue-203:version:1",
        "display": "issue-203"
    })
}

fn input_for_operation(operation: Operation) -> serde_json::Value {
    match operation {
        Operation::ReadTicket => json!({"reference": "203"}),
        Operation::CreateTicket => json!({"title": "title", "body": "body"}),
        Operation::ClaimWorkUnit => json!({"handle": handle(203)}),
        Operation::RecordProgress => json!({"handle": handle(203), "body": "progress"}),
        Operation::DeliverChangeProposal => {
            json!({"work_unit": handle(203), "branch": "issue-203", "commit": "abc123", "base": "main", "summary": "summary", "body": "body", "version": 1})
        }
        Operation::ReflectDisposition => {
            json!({"work_unit": handle(203), "change": change(), "disposition": {"kind": "approved", "against_version": 1, "reviewer": "reviewer", "reviewed_at": "2026-06-17T00:00:00Z", "findings": []}, "body": "approved"})
        }
        Operation::ApplyApprovedChange => {
            json!({"work_unit": handle(203), "change": change(), "approved_version": 1, "approved_commit": "abc123", "base": "main"})
        }
        Operation::CloseOut => {
            json!({"work_unit": handle(203), "completion": {"criterion_summary": "done", "gaps": [], "change_reference": "abc123", "documentation_status": "updated"}, "body": "done"})
        }
    }
}

fn config_with_remote(api_base: &str, git_remote: String) -> SourcehutConfig {
    SourcehutConfig {
        git_remote,
        ..config(api_base)
    }
}

fn git(args: &[&str], cwd: &Path) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|error| panic!("failed to run git {args:?}: {error}"));
    assert!(
        output.status.success(),
        "git {args:?} failed with status {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn local_git_fixture() -> (tempfile::TempDir, String) {
    let remote = tempfile::tempdir().unwrap();
    git(&["init", "--bare"], remote.path());
    let commit = git(
        &["rev-parse", "HEAD"],
        Path::new(env!("CARGO_MANIFEST_DIR")),
    );
    (remote, commit)
}

fn validate_sourcehut_graphql(query: &str) {
    let schema =
        Schema::parse_and_validate(SOURCEHUT_TODO_SCHEMA, "todo.sr.ht.schema.graphqls").unwrap();
    ExecutableDocument::parse_and_validate(&schema, query, "operation.graphql").unwrap();
}

fn graphql_request_from_http(request: &str) -> Value {
    let body = request
        .split("\r\n\r\n")
        .nth(1)
        .expect("HTTP request should include a body");
    serde_json::from_str(body).unwrap_or_else(|error| {
        panic!("request body should be GraphQL JSON: {error}\nrequest:\n{request}")
    })
}

fn assert_graphql_request_validates(request: &str, expected_field: &str) {
    let body = graphql_request_from_http(request);
    let query = body
        .get("query")
        .and_then(Value::as_str)
        .expect("GraphQL request should include a query string");
    validate_sourcehut_graphql(query);
    assert!(
        query.contains(expected_field),
        "query did not contain {expected_field}: {query}"
    );
}

#[test]
fn read_ticket_accepts_deployment_reference_forms_and_issues_scoped_handles() {
    let transport = SourcehutRecordingTransport::with_repeating_response(json!({
        "data": {
            "tracker": {
                "ticket": {
                    "id": 203,
                    "subject": "Forge connectors",
                    "body": "body",
                    "status": "REPORTED"
                }
            }
        }
    }));
    let connector = SourcehutConnector::new(config("https://todo.test/query"), transport.clone());

    for reference in ["sourcehut:4#203", "#203", "203"] {
        let output = connector
            .call(Operation::ReadTicket, json!({ "reference": reference }))
            .unwrap();

        assert_eq!(output["handle"]["id"], "sourcehut:tracker:4:ticket:203");
        assert_eq!(output["title"], "Forge connectors");
    }
}

#[test]
fn foreign_scope_is_rejected_before_transport() {
    let transport = SourcehutRecordingTransport::default();
    let connector = SourcehutConnector::new(config("https://todo.test/query"), transport.clone());

    let error = connector
        .call(
            Operation::CloseOut,
            json!({
                "work_unit": {
                    "id": "sourcehut:tracker:9:ticket:203",
                    "display": "sourcehut:9#203"
                },
                "completion": {
                    "criterion_summary": "done",
                    "gaps": [],
                    "change_reference": "abc123",
                    "documentation_status": "updated"
                },
                "body": "done"
            }),
        )
        .unwrap_err();

    assert!(error.to_string().contains("foreign scope"));
    assert!(transport.requests().is_empty());
}

#[test]
fn every_operation_constructs_the_expected_provider_request() {
    let transport = SourcehutRecordingTransport::with_repeating_response(json!({
        "data": {
            "tracker": {
                "ticket": {
                    "id": 203,
                    "subject": "Forge connectors",
                    "body": "body",
                    "status": "REPORTED"
                }
            },
            "submitTicket": {
                "id": 203,
                "subject": "Forge connectors",
                "body": "body",
                "status": "REPORTED"
            },
            "updateTicketStatus": {
                "id": "claim-203"
            },
            "submitComment": {
                "id": "comment-203"
            },
            "closeOut": {
                "id": "closed-203"
            },
        },
        "commit": "abc123",
        "ref": "refs/heads/issue-203"
    }));
    let connector = SourcehutConnector::new(config("https://todo.test/query"), transport.clone());

    let cases = [
        (
            Operation::ReadTicket,
            json!({"reference": "203"}),
            "GRAPHQL",
            "ticket",
        ),
        (
            Operation::CreateTicket,
            json!({"title": "title", "body": "body"}),
            "GRAPHQL",
            "submitTicket",
        ),
        (
            Operation::ClaimWorkUnit,
            json!({"handle": handle(203)}),
            "GRAPHQL",
            "updateTicketStatus",
        ),
        (
            Operation::RecordProgress,
            json!({"handle": handle(203), "body": "progress"}),
            "GRAPHQL",
            "submitComment",
        ),
        (
            Operation::DeliverChangeProposal,
            json!({"work_unit": handle(203), "branch": "issue-203", "commit": "abc123", "base": "main", "summary": "summary", "body": "body", "version": 1}),
            "GIT",
            "deliverChangeProposal",
        ),
        (
            Operation::ReflectDisposition,
            json!({"work_unit": handle(203), "change": {"id": "sourcehut:tracker:4:change:issue-203:version:1", "display": "issue-203"}, "disposition": {"kind": "approved", "against_version": 1, "reviewer": "reviewer", "reviewed_at": "2026-06-17T00:00:00Z", "findings": []}, "body": "approved"}),
            "GRAPHQL",
            "submitComment",
        ),
        (
            Operation::ApplyApprovedChange,
            json!({"work_unit": handle(203), "change": {"id": "sourcehut:tracker:4:change:issue-203:version:1", "display": "issue-203"}, "approved_version": 1, "approved_commit": "abc123", "base": "main"}),
            "GIT",
            "applyApprovedChange",
        ),
        (
            Operation::CloseOut,
            json!({"work_unit": handle(203), "completion": {"criterion_summary": "done", "gaps": [], "change_reference": "abc123", "documentation_status": "updated"}, "body": "done"}),
            "GRAPHQL",
            "submitComment",
        ),
    ];

    for (operation, input, _, _) in &cases {
        connector.call(*operation, input.clone()).unwrap();
    }

    let requests = transport.requests();
    assert_eq!(requests.len(), cases.len());
    for (request, (_, _, kind, operation_name)) in requests.iter().zip(cases) {
        assert_eq!(request.kind, kind);
        assert_eq!(request.operation, operation_name);
        if request.kind == "GRAPHQL" {
            let body = request
                .body
                .as_ref()
                .expect("GraphQL request should have body");
            let query = body
                .get("query")
                .and_then(Value::as_str)
                .expect("GraphQL request should include query string");
            validate_sourcehut_graphql(query);
            assert!(
                query.contains(operation_name),
                "query did not contain {operation_name}: {query}"
            );
        }
    }
}

#[test]
fn every_operation_rejects_provider_error_payloads_without_success_receipts() {
    let transport = SourcehutRecordingTransport::with_repeating_response(json!({
        "errors": [{"message": "provider rejected mutation"}]
    }));
    let connector = SourcehutConnector::new(config("https://todo.test/query"), transport.clone());

    for operation in Operation::ALL {
        let result = connector.call(operation, input_for_operation(operation));

        assert!(
            result.is_err(),
            "{operation} should reject SourceHut provider errors, got {result:?}"
        );
    }

    assert_eq!(transport.requests().len(), Operation::ALL.len());
}

#[test]
fn every_operation_rejects_null_or_absent_required_provider_results() {
    for response in [
        json!({
            "data": {
                "tracker": {
                    "ticket": null
                },
                "submitTicket": null,
                "updateTicketStatus": null,
                "submitComment": null
            }
        }),
        json!({ "data": {} }),
    ] {
        let transport = SourcehutRecordingTransport::with_repeating_response(response);
        let connector =
            SourcehutConnector::new(config("https://todo.test/query"), transport.clone());

        for operation in Operation::ALL {
            let result = connector.call(operation, input_for_operation(operation));

            assert!(
                result.is_err(),
                "{operation} should reject absent/null required provider result, got {result:?}"
            );
        }
    }
}

#[test]
fn production_http_transport_rejects_graphql_errors_under_http_200() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        let size = stream.read(&mut request).unwrap();
        let request = String::from_utf8_lossy(&request[..size]);
        assert!(request.starts_with("POST /query "));
        assert_graphql_request_validates(&request, "tracker");
        let body = r#"{"errors":[{"message":"provider rejected mutation"}]}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });

    let connector = SourcehutConnector::new(
        config(&format!("http://{address}/query")),
        SourcehutHttpTransport,
    );
    let error = connector
        .call(Operation::ReadTicket, json!({ "reference": "203" }))
        .unwrap_err();

    assert!(error.to_string().contains("GraphQL"));
    server.join().unwrap();
}

#[test]
fn sourcehut_schema_rejects_nonexistent_top_level_ticket_operations() {
    let schema =
        Schema::parse_and_validate(SOURCEHUT_TODO_SCHEMA, "todo.sr.ht.schema.graphqls").unwrap();
    for query in [
        "query ticket($id: Int!) { ticket(id: $id) { id subject body status } }",
        "mutation createTicket($subject: String!, $body: String!) { createTicket(subject: $subject, body: $body) { id subject body status } }",
        "mutation closeTicket($id: Int!, $body: String!) { closeTicket(id: $id, body: $body) { id } }",
    ] {
        assert!(
            ExecutableDocument::parse_and_validate(&schema, query, "operation.graphql").is_err(),
            "query should fail against real SourceHut schema: {query}"
        );
    }
}

#[test]
fn production_http_transport_executes_and_parses_read_ticket() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        let size = stream.read(&mut request).unwrap();
        let request = String::from_utf8_lossy(&request[..size]);
        assert!(request.starts_with("POST /query "));
        assert_graphql_request_validates(&request, "tracker");
        let body = r#"{"data":{"tracker":{"ticket":{"id":203,"subject":"Harness title","body":"Harness body","status":"REPORTED"}}}}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });

    let connector = SourcehutConnector::new(
        config(&format!("http://{address}/query")),
        SourcehutHttpTransport,
    );
    let output = connector
        .call(Operation::ReadTicket, json!({ "reference": "203" }))
        .unwrap();

    assert_eq!(output["title"], "Harness title");
    assert_eq!(output["handle"]["id"], "sourcehut:tracker:4:ticket:203");
    server.join().unwrap();
}

#[test]
fn production_http_transport_executes_and_parses_every_graphql_operation() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let expected = [
        (
            "tracker",
            r#"{"data":{"tracker":{"ticket":{"id":203,"subject":"Harness read","body":"Read body","status":"REPORTED"}}}}"#,
        ),
        (
            "submitTicket",
            r#"{"data":{"submitTicket":{"id":204,"subject":"Harness create","body":"Created body","status":"REPORTED"}}}"#,
        ),
        (
            "updateTicketStatus",
            r#"{"data":{"updateTicketStatus":{"id":"claim-203"}}}"#,
        ),
        (
            "submitComment",
            r#"{"data":{"submitComment":{"id":"progress-1"}}}"#,
        ),
        (
            "submitComment",
            r#"{"data":{"submitComment":{"id":"disposition-1"}}}"#,
        ),
        (
            "submitComment",
            r#"{"data":{"submitComment":{"id":"closed-203"}}}"#,
        ),
    ];
    let server = thread::spawn(move || {
        for (operation_name, body) in expected {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let size = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..size]);
            assert!(
                request.starts_with("POST /query "),
                "unexpected request: {request}"
            );
            assert_graphql_request_validates(&request, operation_name);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        }
    });

    let connector = SourcehutConnector::new(
        config(&format!("http://{address}/query")),
        SourcehutHttpTransport,
    );
    let read = connector
        .call(Operation::ReadTicket, json!({ "reference": "203" }))
        .unwrap();
    let created = connector
        .call(
            Operation::CreateTicket,
            json!({"title": "title", "body": "body"}),
        )
        .unwrap();
    let claimed = connector
        .call(Operation::ClaimWorkUnit, json!({"handle": handle(203)}))
        .unwrap();
    let progress = connector
        .call(
            Operation::RecordProgress,
            json!({"handle": handle(203), "body": "progress"}),
        )
        .unwrap();
    let disposition = connector
        .call(
            Operation::ReflectDisposition,
            json!({"work_unit": handle(203), "change": {"id": "sourcehut:tracker:4:change:issue-203:version:1", "display": "issue-203"}, "disposition": {"kind": "approved", "against_version": 1, "reviewer": "reviewer", "reviewed_at": "2026-06-17T00:00:00Z", "findings": []}, "body": "approved"}),
        )
        .unwrap();
    let closed = connector
        .call(
            Operation::CloseOut,
            json!({"work_unit": handle(203), "completion": {"criterion_summary": "done", "gaps": [], "change_reference": "abc123", "documentation_status": "updated"}, "body": "done"}),
        )
        .unwrap();

    assert_eq!(read["title"], "Harness read");
    assert_eq!(created["handle"]["id"], "sourcehut:tracker:4:ticket:204");
    assert_eq!(created["title"], "Harness create");
    assert_eq!(claimed["receipt"], "claim-203");
    assert_eq!(progress["receipt"], "progress-1");
    assert_eq!(disposition["receipt"], "disposition-1");
    assert_eq!(closed["receipt"], "closed-203");
    server.join().unwrap();
}

#[test]
fn production_git_transport_delivers_change_proposal_to_remote_ref() {
    let (remote, commit) = local_git_fixture();
    let connector = SourcehutConnector::new(
        config_with_remote(
            "http://127.0.0.1:1/query",
            remote.path().to_string_lossy().into_owned(),
        ),
        SourcehutHttpTransport,
    );

    let output = connector
        .call(
            Operation::DeliverChangeProposal,
            json!({
                "work_unit": handle(203),
                "branch": "issue-203",
                "commit": commit,
                "base": "main",
                "summary": "summary",
                "body": "body",
                "version": 1
            }),
        )
        .unwrap();

    let remote_commit = git(
        &[
            "ls-remote",
            remote.path().to_str().unwrap(),
            "refs/heads/issue-203",
        ],
        Path::new(env!("CARGO_MANIFEST_DIR")),
    );
    assert!(remote_commit.starts_with(&commit));
    assert_eq!(output["commit"], commit);
}

#[test]
fn production_git_transport_applies_approved_change_to_base_ref() {
    let (remote, commit) = local_git_fixture();
    let connector = SourcehutConnector::new(
        config_with_remote(
            "http://127.0.0.1:1/query",
            remote.path().to_string_lossy().into_owned(),
        ),
        SourcehutHttpTransport,
    );

    let output = connector
        .call(
            Operation::ApplyApprovedChange,
            json!({
                "work_unit": handle(203),
                "change": {"id": "sourcehut:tracker:4:change:issue-203:version:1", "display": "issue-203"},
                "approved_version": 1,
                "approved_commit": commit,
                "base": "main"
            }),
        )
        .unwrap();

    let remote_commit = git(
        &[
            "ls-remote",
            remote.path().to_str().unwrap(),
            "refs/heads/main",
        ],
        Path::new(env!("CARGO_MANIFEST_DIR")),
    );
    assert!(remote_commit.starts_with(&commit));
    assert_eq!(output["applied_commit"], commit);
    assert_eq!(output["receipt"], "refs/heads/main");
}

#[allow(dead_code)]
fn _request_type_is_public(_: ProviderRequest) {}
