use runa_forge_contract::Operation;
use runa_forge_sourcehut::{
    ProviderRequest, SourcehutConfig, SourcehutConnector, SourcehutHttpTransport,
    SourcehutRecordingTransport,
};
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::thread;

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

#[test]
fn read_ticket_accepts_deployment_reference_forms_and_issues_scoped_handles() {
    let transport = SourcehutRecordingTransport::with_repeating_response(json!({
        "data": {
            "ticket": {
                "id": 203,
                "subject": "Forge connectors",
                "description": "body",
                "status": "open"
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
            "ticket": {
                "id": 203,
                "subject": "Forge connectors",
                "description": "body",
                "status": "open"
            },
            "createTicket": {
                "id": 203,
                "subject": "Forge connectors",
                "description": "body",
                "status": "open"
            }
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
            "createTicket",
        ),
        (
            Operation::ClaimWorkUnit,
            json!({"handle": handle(203)}),
            "GRAPHQL",
            "claimWorkUnit",
        ),
        (
            Operation::RecordProgress,
            json!({"handle": handle(203), "body": "progress"}),
            "GRAPHQL",
            "recordProgress",
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
            "reflectDisposition",
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
            "closeOut",
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
        assert!(request.contains("ticket"));
        let body = r#"{"data":{"ticket":{"id":203,"subject":"Harness title","description":"Harness body","status":"open"}}}"#;
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
            "ticket",
            r#"{"data":{"ticket":{"id":203,"subject":"Harness read","description":"Read body","status":"open"}}}"#,
        ),
        (
            "createTicket",
            r#"{"data":{"createTicket":{"id":204,"subject":"Harness create","description":"Created body","status":"open"}}}"#,
        ),
        (
            "claimWorkUnit",
            r#"{"data":{"updateTicket":{"id":"claim-203"}}}"#,
        ),
        (
            "recordProgress",
            r#"{"data":{"createComment":{"id":"progress-1"}}}"#,
        ),
        (
            "reflectDisposition",
            r#"{"data":{"createComment":{"id":"disposition-1"}}}"#,
        ),
        (
            "closeOut",
            r#"{"data":{"closeTicket":{"id":"closed-203"}}}"#,
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
            assert!(
                request.contains(operation_name),
                "request did not contain {operation_name}: {request}"
            );
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
