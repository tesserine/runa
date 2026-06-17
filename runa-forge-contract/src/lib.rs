use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::fmt;

pub const CAPABILITY_NAME: &str = "forge";
pub const CAPABILITY_VERSION: &str = "1.1.0";
pub const COMMONS_PROVENANCE: &str = "commons@6924159fc4ff58745f0e2c68ed16849ffd9b4086";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Operation {
    ReadTicket,
    CreateTicket,
    ClaimWorkUnit,
    RecordProgress,
    DeliverChangeProposal,
    ReflectDisposition,
    ApplyApprovedChange,
    CloseOut,
}

impl Operation {
    pub const ALL: [Operation; 8] = [
        Operation::ReadTicket,
        Operation::CreateTicket,
        Operation::ClaimWorkUnit,
        Operation::RecordProgress,
        Operation::DeliverChangeProposal,
        Operation::ReflectDisposition,
        Operation::ApplyApprovedChange,
        Operation::CloseOut,
    ];

    pub const fn canonical_name(self) -> &'static str {
        match self {
            Operation::ReadTicket => "read-ticket",
            Operation::CreateTicket => "create-ticket",
            Operation::ClaimWorkUnit => "claim-work-unit",
            Operation::RecordProgress => "record-progress",
            Operation::DeliverChangeProposal => "deliver-change-proposal",
            Operation::ReflectDisposition => "reflect-disposition",
            Operation::ApplyApprovedChange => "apply-approved-change",
            Operation::CloseOut => "close-out",
        }
    }
}

impl fmt::Display for Operation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.canonical_name())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handle {
    pub id: String,
    pub display: String,
}

#[derive(Debug, Clone)]
pub struct ToolDefinition {
    pub set_name: String,
    pub operation: Operation,
    pub name: String,
    pub input_schema: Value,
    pub output_schema: Value,
}

#[derive(Debug, Clone)]
pub struct ForgeToolSet {
    pub set_name: String,
    pub tools: Vec<ToolDefinition>,
}

#[derive(Debug, Clone)]
pub struct ComposedTool {
    pub exposed_name: String,
    pub original_name: String,
    pub set_name: String,
    pub operation: Operation,
    pub input_schema: Value,
    pub output_schema: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompositionError {
    MissingOperation {
        set_name: String,
        operation: Operation,
    },
    DuplicateOperation {
        set_name: String,
        operation: Operation,
    },
    ToolNameCollision {
        tool_name: String,
        first_set: String,
        second_set: String,
    },
}

impl fmt::Display for CompositionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompositionError::MissingOperation {
                set_name,
                operation,
            } => write!(
                f,
                "tool-set '{set_name}' is missing operation '{operation}'"
            ),
            CompositionError::DuplicateOperation {
                set_name,
                operation,
            } => write!(
                f,
                "tool-set '{set_name}' exposes operation '{operation}' more than once"
            ),
            CompositionError::ToolNameCollision {
                tool_name,
                first_set,
                second_set,
            } => write!(
                f,
                "tool name '{tool_name}' is exposed by both '{first_set}' and '{second_set}'"
            ),
        }
    }
}

impl std::error::Error for CompositionError {}

pub fn forge_tool_set(set_name: &str) -> ForgeToolSet {
    ForgeToolSet {
        set_name: set_name.to_string(),
        tools: Vec::new(),
    }
}

pub fn compose_tool_sets(
    sets: &[ForgeToolSet],
    aliases: &HashMap<String, String>,
) -> Result<BTreeMap<String, ComposedTool>, CompositionError> {
    let mut composed: BTreeMap<String, ComposedTool> = BTreeMap::new();
    for set in sets {
        validate_tool_set(set)?;
        for tool in &set.tools {
            let alias_key = format!("{}:{}", set.set_name, tool.name);
            let exposed_name = aliases
                .get(&alias_key)
                .cloned()
                .unwrap_or_else(|| tool.name.clone());
            if let Some(existing) = composed.get(&exposed_name) {
                return Err(CompositionError::ToolNameCollision {
                    tool_name: exposed_name,
                    first_set: existing.set_name.clone(),
                    second_set: set.set_name.clone(),
                });
            }
            composed.insert(
                exposed_name.clone(),
                ComposedTool {
                    exposed_name,
                    original_name: tool.name.clone(),
                    set_name: set.set_name.clone(),
                    operation: tool.operation,
                    input_schema: tool.input_schema.clone(),
                    output_schema: tool.output_schema.clone(),
                },
            );
        }
    }
    Ok(composed)
}

pub fn validate_tool_set(set: &ForgeToolSet) -> Result<(), CompositionError> {
    let mut seen = BTreeMap::new();
    for tool in &set.tools {
        if seen.insert(tool.operation, ()).is_some() {
            return Err(CompositionError::DuplicateOperation {
                set_name: set.set_name.clone(),
                operation: tool.operation,
            });
        }
    }
    for operation in Operation::ALL {
        if !seen.contains_key(&operation) {
            return Err(CompositionError::MissingOperation {
                set_name: set.set_name.clone(),
                operation,
            });
        }
    }
    Ok(())
}

pub fn canonical_forge_tool_set(set_name: &str) -> ForgeToolSet {
    ForgeToolSet {
        set_name: set_name.to_string(),
        tools: Operation::ALL
            .into_iter()
            .map(|operation| ToolDefinition {
                set_name: set_name.to_string(),
                operation,
                name: operation.canonical_name().to_string(),
                input_schema: operation_input_schema(operation),
                output_schema: operation_output_schema(operation),
            })
            .collect(),
    }
}

pub fn operation_input_schema(operation: Operation) -> Value {
    schema_document(operation_input_properties(operation))
}

pub fn operation_output_schema(operation: Operation) -> Value {
    schema_document(operation_output_properties(operation))
}

fn schema_document((required, properties): (Vec<&str>, BTreeMap<&str, Value>)) -> Value {
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "required": required,
        "additionalProperties": false,
        "properties": properties,
        "$defs": {
            "handle": {
                "type": "object",
                "required": ["id", "display"],
                "additionalProperties": false,
                "properties": {
                    "id": { "type": "string", "minLength": 1 },
                    "display": { "type": "string", "minLength": 1 }
                }
            },
            "disposition": {
                "type": "object",
                "required": ["kind", "against_version", "reviewer", "reviewed_at", "findings"],
                "additionalProperties": false,
                "properties": {
                    "kind": { "type": "string", "enum": ["approved", "needs-revision"] },
                    "against_version": { "type": "integer", "minimum": 1 },
                    "reviewer": { "type": "string", "minLength": 1 },
                    "reviewed_at": { "type": "string", "minLength": 1 },
                    "findings": { "type": "array", "items": { "type": "object" } }
                }
            },
            "completion": {
                "type": "object",
                "required": ["criterion_summary", "gaps", "change_reference", "documentation_status"],
                "additionalProperties": false,
                "properties": {
                    "criterion_summary": { "type": "string", "minLength": 1 },
                    "gaps": { "type": "array", "items": { "type": "string", "minLength": 1 } },
                    "change_reference": { "type": "string", "minLength": 1 },
                    "documentation_status": { "type": "string", "minLength": 1 }
                }
            }
        }
    })
}

fn handle_ref() -> Value {
    serde_json::json!({ "$ref": "#/$defs/handle" })
}

fn string_schema() -> Value {
    serde_json::json!({ "type": "string", "minLength": 1 })
}

fn integer_schema() -> Value {
    serde_json::json!({ "type": "integer", "minimum": 1 })
}

fn operation_input_properties(
    operation: Operation,
) -> (Vec<&'static str>, BTreeMap<&'static str, Value>) {
    let mut properties = BTreeMap::new();
    match operation {
        Operation::ReadTicket => {
            properties.insert("reference", string_schema());
            (vec!["reference"], properties)
        }
        Operation::CreateTicket => {
            properties.insert("title", string_schema());
            properties.insert("body", string_schema());
            (vec!["title", "body"], properties)
        }
        Operation::ClaimWorkUnit => {
            properties.insert("handle", handle_ref());
            (vec!["handle"], properties)
        }
        Operation::RecordProgress => {
            properties.insert("handle", handle_ref());
            properties.insert("body", string_schema());
            (vec!["handle", "body"], properties)
        }
        Operation::DeliverChangeProposal => {
            properties.insert("work_unit", handle_ref());
            properties.insert("branch", string_schema());
            properties.insert("commit", string_schema());
            properties.insert("base", string_schema());
            properties.insert("summary", string_schema());
            properties.insert("body", string_schema());
            properties.insert("version", integer_schema());
            (
                vec![
                    "work_unit",
                    "branch",
                    "commit",
                    "base",
                    "summary",
                    "body",
                    "version",
                ],
                properties,
            )
        }
        Operation::ReflectDisposition => {
            properties.insert("work_unit", handle_ref());
            properties.insert("change", handle_ref());
            properties.insert(
                "disposition",
                serde_json::json!({ "$ref": "#/$defs/disposition" }),
            );
            properties.insert("body", string_schema());
            (
                vec!["work_unit", "change", "disposition", "body"],
                properties,
            )
        }
        Operation::ApplyApprovedChange => {
            properties.insert("work_unit", handle_ref());
            properties.insert("change", handle_ref());
            properties.insert("approved_version", integer_schema());
            properties.insert("approved_commit", string_schema());
            properties.insert("base", string_schema());
            (
                vec![
                    "work_unit",
                    "change",
                    "approved_version",
                    "approved_commit",
                    "base",
                ],
                properties,
            )
        }
        Operation::CloseOut => {
            properties.insert("work_unit", handle_ref());
            properties.insert(
                "completion",
                serde_json::json!({ "$ref": "#/$defs/completion" }),
            );
            properties.insert("body", string_schema());
            (vec!["work_unit", "completion", "body"], properties)
        }
    }
}

fn operation_output_properties(
    operation: Operation,
) -> (Vec<&'static str>, BTreeMap<&'static str, Value>) {
    let mut properties = BTreeMap::new();
    match operation {
        Operation::ReadTicket | Operation::CreateTicket => {
            properties.insert("handle", handle_ref());
            properties.insert("title", string_schema());
            properties.insert("body", serde_json::json!({ "type": ["string", "null"] }));
            properties.insert("state", string_schema());
            (vec!["handle", "title", "state"], properties)
        }
        Operation::ClaimWorkUnit | Operation::RecordProgress | Operation::CloseOut => {
            properties.insert("handle", handle_ref());
            properties.insert("receipt", string_schema());
            (vec!["handle", "receipt"], properties)
        }
        Operation::DeliverChangeProposal => {
            properties.insert("handle", handle_ref());
            properties.insert("work_unit", handle_ref());
            properties.insert("commit", string_schema());
            properties.insert("version", integer_schema());
            (vec!["handle", "work_unit", "commit", "version"], properties)
        }
        Operation::ReflectDisposition => {
            properties.insert("work_unit", handle_ref());
            properties.insert("change", handle_ref());
            properties.insert("receipt", string_schema());
            (vec!["work_unit", "change", "receipt"], properties)
        }
        Operation::ApplyApprovedChange => {
            properties.insert("work_unit", handle_ref());
            properties.insert("change", handle_ref());
            properties.insert("applied_commit", string_schema());
            properties.insert("receipt", string_schema());
            (
                vec!["work_unit", "change", "applied_commit", "receipt"],
                properties,
            )
        }
    }
}

pub trait ForgeConnector: Send + Sync {
    fn set_name(&self) -> &str;
    fn tool_set(&self) -> ForgeToolSet {
        canonical_forge_tool_set(self.set_name())
    }
    fn call(&self, operation: Operation, input: Value) -> Result<Value, ForgeError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForgeError {
    InvalidInput(String),
    ForeignScope(String),
    Transport(String),
    ProviderResponse(String),
    Unsupported(String),
}

impl fmt::Display for ForgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ForgeError::InvalidInput(message) => write!(f, "invalid input: {message}"),
            ForgeError::ForeignScope(message) => write!(f, "foreign scope: {message}"),
            ForgeError::Transport(message) => write!(f, "transport error: {message}"),
            ForgeError::ProviderResponse(message) => {
                write!(f, "provider response error: {message}")
            }
            ForgeError::Unsupported(message) => write!(f, "unsupported: {message}"),
        }
    }
}

impl std::error::Error for ForgeError {}
