use runa_forge::{ConnectorConfig, CredentialSource, ForgeConnector, Operation, json_args};
use runa_forge_sourcehut::SourcehutConnector;

fn connector() -> SourcehutConnector {
    SourcehutConnector::new(ConnectorConfig::sourcehut(
        "operator",
        "weforge",
        4,
        "weforge.build",
        CredentialSource::None,
    ))
    .unwrap()
}

#[test]
fn accepts_all_canonical_sourcehut_ticket_reference_forms() {
    for reference in ["sourcehut:4#203", "#203", "203"] {
        let output = connector()
            .dry_run(
                Operation::ReadTicket,
                json_args!({ "reference": reference }),
            )
            .expect("canonical reference should resolve");
        assert_eq!(output["handle"]["id"], "sourcehut:tracker:4:ticket:203");
        assert_eq!(output["handle"]["display"], "sourcehut:4#203");
    }
}

#[test]
fn rejects_foreign_sourcehut_reference_before_transport() {
    let error = connector()
        .dry_run(
            Operation::ReadTicket,
            json_args!({ "reference": "sourcehut:99#203" }),
        )
        .unwrap_err();

    assert!(error.to_string().contains("foreign scope"), "{error}");
}
