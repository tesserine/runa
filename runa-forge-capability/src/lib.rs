use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const CAPABILITY: &str = "forge";
pub const VERSION: &str = "1.0.0";
pub const COMMONS_PROVENANCE: &str = "tesserine/commons@f58ee912b226a8db31902630205bc5ee50b5ee34";
pub const COMMONS_SCHEMA_URL: &str = "https://raw.githubusercontent.com/tesserine/commons/f58ee912b226a8db31902630205bc5ee50b5ee34/schemas/forge-capability/v1/forge-capability.schema.json";

const SCHEMA_JSON: &str = include_str!("../vendor/forge-capability.schema.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ForgeOperation {
    ReadTicket,
    CreateTicket,
    ClaimWorkUnit,
    RecordProgress,
    DeliverChangeProposal,
    ReflectDisposition,
    ApplyApprovedChange,
    CloseOut,
}

impl ForgeOperation {
    pub const ALL: [ForgeOperation; 8] = [
        ForgeOperation::ReadTicket,
        ForgeOperation::CreateTicket,
        ForgeOperation::ClaimWorkUnit,
        ForgeOperation::RecordProgress,
        ForgeOperation::DeliverChangeProposal,
        ForgeOperation::ReflectDisposition,
        ForgeOperation::ApplyApprovedChange,
        ForgeOperation::CloseOut,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ForgeOperation::ReadTicket => "read-ticket",
            ForgeOperation::CreateTicket => "create-ticket",
            ForgeOperation::ClaimWorkUnit => "claim-work-unit",
            ForgeOperation::RecordProgress => "record-progress",
            ForgeOperation::DeliverChangeProposal => "deliver-change-proposal",
            ForgeOperation::ReflectDisposition => "reflect-disposition",
            ForgeOperation::ApplyApprovedChange => "apply-approved-change",
            ForgeOperation::CloseOut => "close-out",
        }
    }

    pub fn input_ref(self) -> &'static str {
        match self {
            ForgeOperation::ReadTicket => "#/$defs/read-ticket-input",
            ForgeOperation::CreateTicket => "#/$defs/create-ticket-input",
            ForgeOperation::ClaimWorkUnit => "#/$defs/claim-work-unit-input",
            ForgeOperation::RecordProgress => "#/$defs/record-progress-input",
            ForgeOperation::DeliverChangeProposal => "#/$defs/deliver-change-proposal-input",
            ForgeOperation::ReflectDisposition => "#/$defs/reflect-disposition-input",
            ForgeOperation::ApplyApprovedChange => "#/$defs/apply-approved-change-input",
            ForgeOperation::CloseOut => "#/$defs/close-out-input",
        }
    }

    pub fn output_ref(self) -> &'static str {
        match self {
            ForgeOperation::ReadTicket | ForgeOperation::CreateTicket => "#/$defs/ticket-snapshot",
            ForgeOperation::ClaimWorkUnit | ForgeOperation::RecordProgress => {
                "#/$defs/work-unit-effect"
            }
            ForgeOperation::DeliverChangeProposal => "#/$defs/change-proposal",
            ForgeOperation::ReflectDisposition => "#/$defs/disposition-effect",
            ForgeOperation::ApplyApprovedChange => "#/$defs/apply-result",
            ForgeOperation::CloseOut => "#/$defs/close-out-effect",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|operation| operation.name() == name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handle {
    pub id: String,
    pub display: String,
}

#[derive(Debug, Clone)]
pub struct ForgeTool {
    pub operation: ForgeOperation,
    pub name: String,
    pub description: String,
    pub input_schema: Map<String, Value>,
    pub output_schema: Map<String, Value>,
}

#[derive(Debug, Clone)]
pub struct ForgeToolSet {
    pub label: String,
    pub tools: Vec<ForgeTool>,
}

#[derive(Debug)]
pub struct ForgeError {
    message: String,
}

impl ForgeError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ForgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ForgeError {}

pub trait ForgeConnector: Send + Sync {
    fn provider(&self) -> &'static str;
    fn tool_set(&self) -> ForgeToolSet;
    fn call(&self, operation: ForgeOperation, input: Value) -> Result<Value, ForgeError>;
}

pub fn capability_schema() -> Value {
    serde_json::from_str(SCHEMA_JSON).expect("vendored forge capability schema must be valid JSON")
}

pub fn capability_descriptor() -> Value {
    json!({
        "capability": CAPABILITY,
        "version": VERSION,
        "handle_schema": "#/$defs/handle",
        "tools": ForgeOperation::ALL.iter().map(|operation| {
            json!({
                "name": operation.name(),
                "input_schema": operation.input_ref(),
                "output_schema": operation.output_ref(),
            })
        }).collect::<Vec<_>>()
    })
}

pub fn validate_capability_descriptor() -> Result<(), String> {
    let schema = capability_schema();
    let validator = jsonschema::validator_for(&schema).map_err(|error| error.to_string())?;
    let descriptor = capability_descriptor();
    let violations = validator
        .iter_errors(&descriptor)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations.join("\n"))
    }
}

pub fn schema_for_ref(schema_ref: &str) -> Result<Map<String, Value>, String> {
    let name = schema_ref
        .strip_prefix("#/$defs/")
        .ok_or_else(|| format!("unsupported schema ref: {schema_ref}"))?;
    let schema = capability_schema();
    let value = schema
        .get("$defs")
        .and_then(|defs| defs.get(name))
        .ok_or_else(|| format!("schema ref not found: {schema_ref}"))?
        .clone();
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(format!("schema ref is not an object: {schema_ref}")),
    }
}

pub fn validate_value(schema: &Map<String, Value>, data: &Value) -> Result<(), String> {
    let schema = Value::Object(schema.clone());
    let validator = jsonschema::validator_for(&schema).map_err(|error| error.to_string())?;
    let violations = validator
        .iter_errors(data)
        .map(|error| format!("{}: {}", error.instance_path(), error))
        .collect::<Vec<_>>();
    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations.join("\n"))
    }
}

pub fn canonical_tool_set(label: impl Into<String>) -> ForgeToolSet {
    ForgeToolSet {
        label: label.into(),
        tools: ForgeOperation::ALL
            .iter()
            .map(|operation| ForgeTool {
                operation: *operation,
                name: operation.name().to_string(),
                description: format!("Forge operation: {}", operation.name()),
                input_schema: schema_for_ref(operation.input_ref())
                    .expect("operation input schema ref must exist"),
                output_schema: schema_for_ref(operation.output_ref())
                    .expect("operation output schema ref must exist"),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_matches_vendored_commons_schema() {
        validate_capability_descriptor().unwrap();
        assert_eq!(
            capability_schema()["$id"],
            Value::String(COMMONS_SCHEMA_URL.to_string())
        );
    }

    #[test]
    fn every_operation_schema_ref_resolves_to_an_object_schema() {
        for operation in ForgeOperation::ALL {
            assert!(
                schema_for_ref(operation.input_ref())
                    .unwrap()
                    .contains_key("type")
            );
            assert!(
                schema_for_ref(operation.output_ref())
                    .unwrap()
                    .contains_key("type")
            );
        }
    }
}
