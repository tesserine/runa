use jsonschema::Validator;
use runa_forge_contract::{
    CompositionError, Operation, canonical_forge_tool_set, compose_tool_sets,
    operation_input_schema, operation_output_schema, validate_tool_set,
};
use serde_json::json;
use std::collections::HashMap;

#[test]
fn canonical_tool_set_exposes_all_v1_1_operations() {
    let set = canonical_forge_tool_set("github");

    validate_tool_set(&set).unwrap();
    assert_eq!(set.tools.len(), 8);
    for operation in Operation::ALL {
        assert!(
            set.tools.iter().any(|tool| {
                tool.operation == operation && tool.name == operation.canonical_name()
            }),
            "missing {operation}"
        );
    }
}

#[test]
fn emitted_operation_schemas_are_self_contained() {
    for operation in Operation::ALL {
        let input_schema = operation_input_schema(operation);
        let output_schema = operation_output_schema(operation);

        Validator::options()
            .build(&input_schema)
            .unwrap_or_else(|error| panic!("{operation} input schema should compile: {error}"));
        Validator::options()
            .build(&output_schema)
            .unwrap_or_else(|error| panic!("{operation} output schema should compile: {error}"));

        assert!(
            input_schema.get("$defs").is_some(),
            "{operation} input schema should include local $defs"
        );
        assert!(
            output_schema.get("$defs").is_some(),
            "{operation} output schema should include local $defs"
        );
    }
}

#[test]
fn composition_reports_colliding_tool_sets_by_name() {
    let github = canonical_forge_tool_set("github");
    let sourcehut = canonical_forge_tool_set("sourcehut");

    let error = compose_tool_sets(&[github, sourcehut], &HashMap::new()).unwrap_err();

    let CompositionError::ToolNameCollision {
        tool_name,
        first_set,
        second_set,
    } = error
    else {
        panic!("expected collision, got {error:?}");
    };
    assert!(
        Operation::ALL
            .iter()
            .any(|operation| operation.canonical_name() == tool_name)
    );
    assert_eq!(first_set, "github");
    assert_eq!(second_set, "sourcehut");
}

#[test]
fn composition_accepts_role_qualified_aliases() {
    let github = canonical_forge_tool_set("github");
    let sourcehut = canonical_forge_tool_set("sourcehut");
    let aliases = Operation::ALL
        .into_iter()
        .map(|operation| {
            (
                format!("sourcehut:{}", operation.canonical_name()),
                format!("work-unit-{}", operation.canonical_name()),
            )
        })
        .collect();

    let composed = compose_tool_sets(&[github, sourcehut], &aliases).unwrap();

    assert!(composed.contains_key("read-ticket"));
    assert!(composed.contains_key("work-unit-read-ticket"));
    assert_eq!(composed.len(), 16);
}

#[test]
fn schemas_validate_representative_payloads() {
    let handle = json!({"id": "github:tesserine/runa:issue:203", "display": "tesserine/runa#203"});
    let cases = [
        (
            Operation::ReadTicket,
            json!({"reference": "tesserine/runa#203"}),
        ),
        (Operation::ClaimWorkUnit, json!({"handle": handle.clone()})),
        (
            Operation::DeliverChangeProposal,
            json!({
                "work_unit": handle,
                "branch": "issue-203/runa-mcp-forge-connectors",
                "commit": "abc123",
                "base": "main",
                "summary": "summary",
                "body": "body",
                "version": 1
            }),
        ),
    ];

    for (operation, payload) in cases {
        let schema = operation_input_schema(operation);
        let validator = Validator::options().build(&schema).unwrap();
        assert!(
            validator.is_valid(&payload),
            "{operation} schema should validate representative payload"
        );
    }
}
