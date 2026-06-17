use runa_forge::{
    FORGE_CAPABILITY_CANONICAL_URL, FORGE_CAPABILITY_SCHEMA, FORGE_CAPABILITY_VERSION, Operation,
    connector_descriptor_schema, forge_operation_descriptors,
};
use serde_json::json;

#[test]
fn vendored_capability_schema_is_pinned_to_commons_v1_1_0() {
    assert_eq!(FORGE_CAPABILITY_VERSION, "1.1.0");
    assert!(FORGE_CAPABILITY_CANONICAL_URL.contains("6924159fc4ff58745f0e2c68ed16849ffd9b4086"));
    assert_eq!(
        FORGE_CAPABILITY_SCHEMA["properties"]["version"]["const"],
        "1.1.0"
    );
}

#[test]
fn forge_descriptor_exposes_exactly_the_eight_v1_operations() {
    let descriptor = forge_operation_descriptors("github");
    let operations: Vec<_> = descriptor.tools.iter().map(|tool| tool.operation).collect();

    assert_eq!(
        operations,
        [
            Operation::ReadTicket,
            Operation::CreateTicket,
            Operation::ClaimWorkUnit,
            Operation::RecordProgress,
            Operation::ReflectDisposition,
            Operation::CloseOut,
            Operation::DeliverChangeProposal,
            Operation::ApplyApprovedChange,
        ]
    );

    jsonschema::validator_for(&connector_descriptor_schema())
        .unwrap()
        .validate(&json!(descriptor))
        .expect("connector descriptor should validate against vendored schema");
}
