use runa_forge::{ConnectorConfig, CredentialSource, ForgeConnector, Operation, json_args};
use runa_forge_github::GithubConnector;

fn connector() -> GithubConnector {
    GithubConnector::new(ConnectorConfig::github(
        "tesserine",
        "runa",
        CredentialSource::None,
    ))
    .unwrap()
}

#[test]
fn accepts_all_canonical_github_ticket_reference_forms() {
    for reference in [
        "github:tesserine/runa#203",
        "tesserine/runa#203",
        "#203",
        "203",
    ] {
        let output = connector()
            .dry_run(
                Operation::ReadTicket,
                json_args!({ "reference": reference }),
            )
            .expect("canonical reference should resolve");
        assert_eq!(output["handle"]["id"], "github:tesserine/runa:issue:203");
        assert_eq!(output["handle"]["display"], "github:tesserine/runa#203");
    }
}

#[test]
fn rejects_foreign_github_reference_before_transport() {
    let error = connector()
        .dry_run(
            Operation::ReadTicket,
            json_args!({ "reference": "github:other/repo#203" }),
        )
        .unwrap_err();

    assert!(error.to_string().contains("foreign scope"), "{error}");
}
