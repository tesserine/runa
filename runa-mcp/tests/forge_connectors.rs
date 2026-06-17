use jsonschema::Validator;
use runa_forge_contract::{
    FORGE_CAPABILITY_COMMONS_COMMIT, FORGE_CAPABILITY_VERSION, ForgeOperation, compose_tool_sets,
};
use runa_forge_github::{GitHubConfig, GitHubConnector, RecordingGitHubTransport};
use runa_forge_sourcehut::{RecordingSourceHutTransport, SourceHutConfig, SourceHutConnector};
use serde_json::json;

#[test]
fn forge_connectors_advertise_all_v1_1_operations_with_self_contained_schemas() {
    assert_eq!(FORGE_CAPABILITY_VERSION, "1.1.0");
    assert_eq!(
        FORGE_CAPABILITY_COMMONS_COMMIT,
        "6924159fc4ff58745f0e2c68ed16849ffd9b4086"
    );

    for connector in [
        GitHubConnector::new_for_test(
            GitHubConfig::new("tesserine", "runa"),
            RecordingGitHubTransport::default(),
        )
        .tool_set(),
        SourceHutConnector::new_for_test(
            SourceHutConfig::new("operator", "weforge", 4, "weforge.build", 42),
            RecordingSourceHutTransport::default(),
        )
        .tool_set(),
    ] {
        let operations = connector
            .tools
            .iter()
            .map(|tool| tool.operation)
            .collect::<Vec<_>>();
        assert_eq!(operations, ForgeOperation::ALL);

        for tool in &connector.tools {
            let input = serde_json::Value::Object(tool.input_schema.clone());
            let output = serde_json::Value::Object(tool.output_schema.clone());
            let input_validator = Validator::new(&input)
                .unwrap_or_else(|error| panic!("{} input schema invalid: {error}", tool.name));
            let output_validator = Validator::new(&output)
                .unwrap_or_else(|error| panic!("{} output schema invalid: {error}", tool.name));
            assert!(
                input_validator.is_valid(&tool.representative_input),
                "{} representative input should validate against emitted schema",
                tool.name
            );
            assert!(
                output_validator.is_valid(&tool.representative_output),
                "{} representative output should validate against emitted schema",
                tool.name
            );
        }
    }
}

#[test]
fn forge_tool_set_composition_rejects_unaliased_collisions() {
    let github = GitHubConnector::new_for_test(
        GitHubConfig::new("tesserine", "runa"),
        RecordingGitHubTransport::default(),
    )
    .tool_set();
    let sourcehut = SourceHutConnector::new_for_test(
        SourceHutConfig::new("operator", "weforge", 4, "weforge.build", 42),
        RecordingSourceHutTransport::default(),
    )
    .tool_set();

    let error = compose_tool_sets(vec![github, sourcehut], Default::default()).unwrap_err();
    let message = error.to_string();
    assert!(message.contains("read-ticket"), "{message}");
    assert!(message.contains("github"), "{message}");
    assert!(message.contains("sourcehut"), "{message}");
}

#[test]
fn github_reference_resolution_accepts_canonical_forms_and_rejects_foreign_scope() {
    let connector = GitHubConnector::new_for_test(
        GitHubConfig::new("tesserine", "runa"),
        RecordingGitHubTransport::default(),
    );

    for reference in [
        "github:tesserine/runa#203",
        "tesserine/runa#203",
        "#203",
        "203",
    ] {
        let handle = connector.resolve_reference(reference).unwrap();
        assert_eq!(handle.id, "github:tesserine/runa:issue:203");
        assert_eq!(handle.display, "github:tesserine/runa#203");
    }

    assert!(
        connector
            .resolve_reference("github:tesserine/commons#203")
            .is_err()
    );
    assert!(connector.validate_work_unit_handle_id("203").is_err());
}

#[test]
fn sourcehut_reference_resolution_accepts_canonical_forms_and_rejects_foreign_scope() {
    let connector = SourceHutConnector::new_for_test(
        SourceHutConfig::new("operator", "weforge", 4, "weforge.build", 42),
        RecordingSourceHutTransport::default(),
    );

    for reference in ["sourcehut:4#203", "#203", "203"] {
        let handle = connector.resolve_reference(reference).unwrap();
        assert_eq!(handle.id, "sourcehut:tracker:4:ticket:203");
        assert_eq!(handle.display, "sourcehut:4#203");
    }

    assert!(connector.resolve_reference("sourcehut:99#203").is_err());
    assert!(connector.validate_work_unit_handle_id("203").is_err());
}

#[test]
fn operation_calls_construct_provider_requests_before_returning_outputs() {
    let github_transport = RecordingGitHubTransport::default();
    let github = GitHubConnector::new_for_test(
        GitHubConfig::new("tesserine", "runa"),
        github_transport.clone(),
    );
    github
        .call(
            ForgeOperation::RecordProgress,
            json!({
                "handle": {"id": "github:tesserine/runa:issue:203", "display": "github:tesserine/runa#203"},
                "body": "progress"
            }),
        )
        .unwrap();
    assert_eq!(
        github_transport.take_requests()[0].summary,
        "github POST repos/tesserine/runa/issues/203/comments"
    );

    let sourcehut_transport = RecordingSourceHutTransport::default();
    let sourcehut = SourceHutConnector::new_for_test(
        SourceHutConfig::new("operator", "weforge", 4, "weforge.build", 42),
        sourcehut_transport.clone(),
    );
    sourcehut
        .call(
            ForgeOperation::RecordProgress,
            json!({
                "handle": {"id": "sourcehut:tracker:4:ticket:203", "display": "sourcehut:4#203"},
                "body": "progress"
            }),
        )
        .unwrap();
    assert_eq!(
        sourcehut_transport.take_requests()[0].summary,
        "sourcehut graphql submitComment tracker=4 ticket=203"
    );
}
