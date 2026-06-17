use runa_forge_contract::Operation;
use runa_forge_github::{
    GithubConfig, GithubConnector, GithubHttpTransport, GithubRecordingTransport, ProviderRequest,
};
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

fn config(api_base: &str) -> GithubConfig {
    GithubConfig {
        owner: "tesserine".to_string(),
        repo: "runa".to_string(),
        api_base: api_base.to_string(),
        credential_env: None,
        credential_command: None,
    }
}

fn handle(number: u64) -> serde_json::Value {
    json!({
        "id": format!("github:tesserine/runa:issue:{number}"),
        "display": format!("tesserine/runa#{number}")
    })
}

#[test]
fn read_ticket_accepts_deployment_reference_forms_and_issues_scoped_handles() {
    let transport = GithubRecordingTransport::with_repeating_response(json!({
        "number": 203,
        "title": "Forge connectors",
        "body": "body",
        "state": "open"
    }));
    let connector = GithubConnector::new(config("https://api.github.test"), transport.clone());

    for reference in [
        "github:tesserine/runa#203",
        "tesserine/runa#203",
        "#203",
        "203",
    ] {
        let output = connector
            .call(Operation::ReadTicket, json!({ "reference": reference }))
            .unwrap();

        assert_eq!(output["handle"]["id"], "github:tesserine/runa:issue:203");
        assert_eq!(output["title"], "Forge connectors");
    }
}

#[test]
fn foreign_scope_is_rejected_before_transport() {
    let transport = GithubRecordingTransport::default();
    let connector = GithubConnector::new(config("https://api.github.test"), transport.clone());

    let error = connector
        .call(
            Operation::RecordProgress,
            json!({
                "handle": {
                    "id": "github:tesserine/groundwork:issue:203",
                    "display": "tesserine/groundwork#203"
                },
                "body": "progress"
            }),
        )
        .unwrap_err();

    assert!(error.to_string().contains("foreign scope"));
    assert!(transport.requests().is_empty());
}

#[test]
fn every_operation_constructs_the_expected_provider_request() {
    let transport = GithubRecordingTransport::with_repeating_response(json!({
        "number": 203,
        "title": "Forge connectors",
        "body": "body",
        "state": "open",
        "html_url": "https://github.com/tesserine/runa/pull/12",
        "sha": "abc123",
        "merged": true
    }));
    let connector = GithubConnector::new(config("https://api.github.test"), transport.clone());

    let cases = [
        (
            Operation::ReadTicket,
            json!({"reference": "203"}),
            "GET",
            "/repos/tesserine/runa/issues/203",
        ),
        (
            Operation::CreateTicket,
            json!({"title": "title", "body": "body"}),
            "POST",
            "/repos/tesserine/runa/issues",
        ),
        (
            Operation::ClaimWorkUnit,
            json!({"handle": handle(203)}),
            "POST",
            "/repos/tesserine/runa/issues/203/assignees",
        ),
        (
            Operation::RecordProgress,
            json!({"handle": handle(203), "body": "progress"}),
            "POST",
            "/repos/tesserine/runa/issues/203/comments",
        ),
        (
            Operation::DeliverChangeProposal,
            json!({"work_unit": handle(203), "branch": "issue-203", "commit": "abc123", "base": "main", "summary": "summary", "body": "body", "version": 1}),
            "POST",
            "/repos/tesserine/runa/pulls",
        ),
        (
            Operation::ReflectDisposition,
            json!({"work_unit": handle(203), "change": {"id": "github:tesserine/runa:pull:12:version:1", "display": "tesserine/runa#12"}, "disposition": {"kind": "approved", "against_version": 1, "reviewer": "reviewer", "reviewed_at": "2026-06-17T00:00:00Z", "findings": []}, "body": "approved"}),
            "POST",
            "/repos/tesserine/runa/issues/12/comments",
        ),
        (
            Operation::ApplyApprovedChange,
            json!({"work_unit": handle(203), "change": {"id": "github:tesserine/runa:pull:12:version:1", "display": "tesserine/runa#12"}, "approved_version": 1, "approved_commit": "abc123", "base": "main"}),
            "PUT",
            "/repos/tesserine/runa/pulls/12/merge",
        ),
        (
            Operation::CloseOut,
            json!({"work_unit": handle(203), "completion": {"criterion_summary": "done", "gaps": [], "change_reference": "abc123", "documentation_status": "updated"}, "body": "done"}),
            "PATCH",
            "/repos/tesserine/runa/issues/203",
        ),
    ];

    for (operation, input, _, _) in &cases {
        connector.call(*operation, input.clone()).unwrap();
    }

    let requests = transport.requests();
    assert_eq!(requests.len(), cases.len());
    for (request, (_, _, method, path)) in requests.iter().zip(cases) {
        assert_eq!(request.method, method);
        assert_eq!(request.path, path);
    }
}

#[test]
fn production_http_transport_executes_and_parses_read_ticket() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 2048];
        let size = stream.read(&mut request).unwrap();
        let request = String::from_utf8_lossy(&request[..size]);
        assert!(request.starts_with("GET /repos/tesserine/runa/issues/203 "));
        let body = r#"{"number":203,"title":"Harness title","body":"Harness body","state":"open"}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });

    let connector = GithubConnector::new(config(&format!("http://{address}")), GithubHttpTransport);
    let output = connector
        .call(Operation::ReadTicket, json!({ "reference": "203" }))
        .unwrap();

    assert_eq!(output["title"], "Harness title");
    assert_eq!(output["handle"]["id"], "github:tesserine/runa:issue:203");
    server.join().unwrap();
}

#[allow(dead_code)]
fn _request_type_is_public(_: ProviderRequest) {}
