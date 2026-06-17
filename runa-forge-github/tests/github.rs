use runa_forge_contract::Operation;
use runa_forge_github::{
    GithubConfig, GithubConnector, GithubHttpTransport, GithubRecordingTransport, ProviderRequest,
};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

fn config(api_base: &str) -> GithubConfig {
    GithubConfig {
        owner: "tesserine".to_string(),
        repo: "runa".to_string(),
        api_base: api_base.to_string(),
        assignee: Some("pentaxis93".to_string()),
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

fn change() -> serde_json::Value {
    json!({
        "id": "github:tesserine/runa:pull:12:version:1",
        "display": "tesserine/runa#12"
    })
}

fn input_for_operation(operation: Operation) -> Value {
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

fn github_error_server() -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        let size = stream.read(&mut request).unwrap();
        assert!(size > 0, "expected an HTTP request");
        let request = String::from_utf8_lossy(&request[..size]);
        assert_github_user_agent(&request);
        let body = r#"{"message":"provider rejected request"}"#;
        write!(
            stream,
            "HTTP/1.1 500 Internal Server Error\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });
    (format!("http://{address}"), server)
}

fn assert_github_user_agent(request: &str) {
    let request = request.to_ascii_lowercase();
    assert!(
        request.contains("\r\nuser-agent: runa-forge-github/"),
        "missing runa GitHub User-Agent header: {request}"
    );
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
        "base": { "ref": "main" },
        "head": { "sha": "abc123" },
        "sha": "abc123",
        "merged": true
    }));
    let connector = GithubConnector::new(config("https://api.github.test"), transport.clone());

    let cases = [
        (Operation::ReadTicket, json!({"reference": "203"})),
        (
            Operation::CreateTicket,
            json!({"title": "title", "body": "body"}),
        ),
        (Operation::ClaimWorkUnit, json!({"handle": handle(203)})),
        (
            Operation::RecordProgress,
            json!({"handle": handle(203), "body": "progress"}),
        ),
        (
            Operation::DeliverChangeProposal,
            json!({"work_unit": handle(203), "branch": "issue-203", "commit": "abc123", "base": "main", "summary": "summary", "body": "body", "version": 1}),
        ),
        (
            Operation::ReflectDisposition,
            json!({"work_unit": handle(203), "change": {"id": "github:tesserine/runa:pull:12:version:1", "display": "tesserine/runa#12"}, "disposition": {"kind": "approved", "against_version": 1, "reviewer": "reviewer", "reviewed_at": "2026-06-17T00:00:00Z", "findings": []}, "body": "approved"}),
        ),
        (
            Operation::ApplyApprovedChange,
            json!({"work_unit": handle(203), "change": {"id": "github:tesserine/runa:pull:12:version:1", "display": "tesserine/runa#12"}, "approved_version": 1, "approved_commit": "abc123", "base": "main"}),
        ),
        (
            Operation::CloseOut,
            json!({"work_unit": handle(203), "completion": {"criterion_summary": "done", "gaps": [], "change_reference": "abc123", "documentation_status": "updated"}, "body": "done"}),
        ),
    ];

    for (operation, input) in &cases {
        connector.call(*operation, input.clone()).unwrap();
    }

    let expected_requests = [
        ("GET", "/repos/tesserine/runa/issues/203"),
        ("POST", "/repos/tesserine/runa/issues"),
        ("POST", "/repos/tesserine/runa/issues/203/assignees"),
        ("POST", "/repos/tesserine/runa/issues/203/comments"),
        ("POST", "/repos/tesserine/runa/pulls"),
        ("POST", "/repos/tesserine/runa/issues/12/comments"),
        ("GET", "/repos/tesserine/runa/pulls/12"),
        ("PUT", "/repos/tesserine/runa/pulls/12/merge"),
        ("POST", "/repos/tesserine/runa/issues/203/comments"),
        ("PATCH", "/repos/tesserine/runa/issues/203"),
    ];
    let requests = transport.requests();
    assert_eq!(requests.len(), expected_requests.len());
    for (request, (method, path)) in requests.iter().zip(expected_requests) {
        assert_eq!(request.method, method);
        assert_eq!(request.path, path);
    }
    assert_eq!(requests[9].body, Some(json!({ "state": "closed" })));
}

#[test]
fn claim_work_unit_sends_assignee_payload() {
    let transport = GithubRecordingTransport::with_repeating_response(json!({
        "url": "https://api.github.test/repos/tesserine/runa/issues/203"
    }));
    let connector = GithubConnector::new(config("https://api.github.test"), transport.clone());

    connector
        .call(Operation::ClaimWorkUnit, json!({"handle": handle(203)}))
        .unwrap();

    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body,
        Some(json!({ "assignees": ["pentaxis93"] }))
    );
}

#[test]
fn close_out_posts_note_then_closes_issue_without_overwriting_body() {
    let transport = GithubRecordingTransport::with_repeating_response(json!({
        "html_url": "https://github.test/closed/203"
    }));
    let connector = GithubConnector::new(config("https://api.github.test"), transport.clone());

    connector
        .call(
            Operation::CloseOut,
            json!({
                "work_unit": handle(203),
                "completion": {
                    "criterion_summary": "done",
                    "gaps": [],
                    "change_reference": "abc123",
                    "documentation_status": "updated"
                },
                "body": "closing note"
            }),
        )
        .unwrap();

    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(
        requests[0].path,
        "/repos/tesserine/runa/issues/203/comments"
    );
    assert_eq!(requests[0].body, Some(json!({ "body": "closing note" })));
    assert_eq!(requests[1].method, "PATCH");
    assert_eq!(requests[1].path, "/repos/tesserine/runa/issues/203");
    assert_eq!(requests[1].body, Some(json!({ "state": "closed" })));
}

#[test]
fn apply_approved_change_rejects_base_mismatch_before_merge() {
    let transport = GithubRecordingTransport::with_response(json!({
        "base": { "ref": "develop" },
        "head": { "sha": "abc123" }
    }));
    let connector = GithubConnector::new(config("https://api.github.test"), transport.clone());

    let error = connector
        .call(
            Operation::ApplyApprovedChange,
            json!({
                "work_unit": handle(203),
                "change": {
                    "id": "github:tesserine/runa:pull:12:version:1",
                    "display": "tesserine/runa#12"
                },
                "approved_version": 1,
                "approved_commit": "abc123",
                "base": "main"
            }),
        )
        .unwrap_err();

    assert!(
        error.to_string().contains("base") && error.to_string().contains("main"),
        "base mismatch should be reported: {error}"
    );
    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].path, "/repos/tesserine/runa/pulls/12");
}

#[test]
fn deliver_change_proposal_rejects_created_pr_head_sha_mismatch() {
    let transport = GithubRecordingTransport::with_response(json!({
        "number": 12,
        "head": { "sha": "different-commit" }
    }));
    let connector = GithubConnector::new(config("https://api.github.test"), transport.clone());

    let error = connector
        .call(
            Operation::DeliverChangeProposal,
            json!({
                "work_unit": handle(203),
                "branch": "issue-203",
                "commit": "requested-commit",
                "base": "main",
                "summary": "summary",
                "body": "body",
                "version": 1
            }),
        )
        .unwrap_err();

    assert!(
        error.to_string().contains("requested-commit")
            && error.to_string().contains("different-commit"),
        "head SHA mismatch should report requested and actual commits: {error}"
    );
    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/repos/tesserine/runa/pulls");
}

#[test]
fn every_operation_rejects_http_error_status_without_success_receipts() {
    for operation in Operation::ALL {
        let (api_base, server) = github_error_server();
        let connector = GithubConnector::new(config(&api_base), GithubHttpTransport);

        let result = connector.call(operation, input_for_operation(operation));

        assert!(
            result.is_err(),
            "{operation} should reject GitHub HTTP errors, got {result:?}"
        );
        assert!(
            result.unwrap_err().to_string().contains("500"),
            "{operation} error should report the HTTP status"
        );
        server.join().unwrap();
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
        assert_github_user_agent(&request);
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

#[test]
fn production_http_transport_executes_and_parses_every_operation() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let expected = [
        (
            "GET",
            "/repos/tesserine/runa/issues/203",
            r#"{"number":203,"title":"Harness read","body":"Harness body","state":"open"}"#,
        ),
        (
            "POST",
            "/repos/tesserine/runa/issues",
            r#"{"number":204,"title":"Harness create","body":"Created body","state":"open"}"#,
        ),
        (
            "POST",
            "/repos/tesserine/runa/issues/203/assignees",
            r#"{"url":"https://api.github.test/claim/203"}"#,
        ),
        (
            "POST",
            "/repos/tesserine/runa/issues/203/comments",
            r#"{"html_url":"https://github.test/progress/1"}"#,
        ),
        (
            "POST",
            "/repos/tesserine/runa/pulls",
            r#"{"number":12,"head":{"sha":"input-commit"}}"#,
        ),
        (
            "POST",
            "/repos/tesserine/runa/issues/12/comments",
            r#"{"html_url":"https://github.test/disposition/1"}"#,
        ),
        (
            "GET",
            "/repos/tesserine/runa/pulls/12",
            r#"{"base":{"ref":"main"},"head":{"sha":"input-approved"}}"#,
        ),
        (
            "PUT",
            "/repos/tesserine/runa/pulls/12/merge",
            r#"{"sha":"merged-sha"}"#,
        ),
        (
            "POST",
            "/repos/tesserine/runa/issues/203/comments",
            r#"{"html_url":"https://github.test/close-comment/1"}"#,
        ),
        (
            "PATCH",
            "/repos/tesserine/runa/issues/203",
            r#"{"html_url":"https://github.test/closed/203"}"#,
        ),
    ];
    let server = thread::spawn(move || {
        for (method, path, body) in expected {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let size = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..size]);
            assert!(
                request.starts_with(&format!("{method} {path} ")),
                "unexpected request: {request}"
            );
            assert_github_user_agent(&request);
            if path.ends_with("/assignees") {
                assert!(request.contains(r#""assignees":["pentaxis93"]"#));
            }
            write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        }
    });

    let connector = GithubConnector::new(config(&format!("http://{address}")), GithubHttpTransport);
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
    let delivered = connector
        .call(
            Operation::DeliverChangeProposal,
            json!({"work_unit": handle(203), "branch": "issue-203", "commit": "input-commit", "base": "main", "summary": "summary", "body": "body", "version": 1}),
        )
        .unwrap();
    let disposition = connector
        .call(
            Operation::ReflectDisposition,
            json!({"work_unit": handle(203), "change": {"id": "github:tesserine/runa:pull:12:version:1", "display": "tesserine/runa#12"}, "disposition": {"kind": "approved", "against_version": 1, "reviewer": "reviewer", "reviewed_at": "2026-06-17T00:00:00Z", "findings": []}, "body": "approved"}),
        )
        .unwrap();
    let applied = connector
        .call(
            Operation::ApplyApprovedChange,
            json!({"work_unit": handle(203), "change": {"id": "github:tesserine/runa:pull:12:version:1", "display": "tesserine/runa#12"}, "approved_version": 1, "approved_commit": "input-approved", "base": "main"}),
        )
        .unwrap();
    let closed = connector
        .call(
            Operation::CloseOut,
            json!({"work_unit": handle(203), "completion": {"criterion_summary": "done", "gaps": [], "change_reference": "abc123", "documentation_status": "updated"}, "body": "done"}),
        )
        .unwrap();

    assert_eq!(read["title"], "Harness read");
    assert_eq!(created["handle"]["id"], "github:tesserine/runa:issue:204");
    assert_eq!(created["title"], "Harness create");
    assert_eq!(claimed["receipt"], "https://api.github.test/claim/203");
    assert_eq!(progress["receipt"], "https://github.test/progress/1");
    assert_eq!(delivered["commit"], "input-commit");
    assert_eq!(disposition["receipt"], "https://github.test/disposition/1");
    assert_eq!(applied["applied_commit"], "merged-sha");
    assert_eq!(closed["receipt"], "https://github.test/closed/203");
    server.join().unwrap();
}

#[test]
fn production_http_close_out_preserves_issue_body_and_records_note() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let mut issue_body = "Original problem statement\n\nAcceptance criteria".to_string();
        let mut comments = Vec::new();

        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        let size = stream.read(&mut request).unwrap();
        let request = String::from_utf8_lossy(&request[..size]);
        assert!(request.starts_with("POST /repos/tesserine/runa/issues/203/comments "));
        assert_github_user_agent(&request);
        comments.push("closing note".to_string());
        let body = r#"{"html_url":"https://github.test/comments/1"}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();

        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        let size = stream.read(&mut request).unwrap();
        let request = String::from_utf8_lossy(&request[..size]);
        assert!(request.starts_with("PATCH /repos/tesserine/runa/issues/203 "));
        assert_github_user_agent(&request);
        let patch: serde_json::Value =
            serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(patch, json!({ "state": "closed" }));
        if let Some(new_body) = patch.get("body").and_then(serde_json::Value::as_str) {
            issue_body = new_body.to_string();
        }
        let body = r#"{"html_url":"https://github.test/closed/203"}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();

        assert_eq!(comments, vec!["closing note"]);
        assert_eq!(
            issue_body,
            "Original problem statement\n\nAcceptance criteria"
        );
    });

    let connector = GithubConnector::new(config(&format!("http://{address}")), GithubHttpTransport);
    let output = connector
        .call(
            Operation::CloseOut,
            json!({
                "work_unit": handle(203),
                "completion": {
                    "criterion_summary": "done",
                    "gaps": [],
                    "change_reference": "abc123",
                    "documentation_status": "updated"
                },
                "body": "closing note"
            }),
        )
        .unwrap();

    assert_eq!(output["receipt"], "https://github.test/closed/203");
    server.join().unwrap();
}

#[allow(dead_code)]
fn _request_type_is_public(_: ProviderRequest) {}
