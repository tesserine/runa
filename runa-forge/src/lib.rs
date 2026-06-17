//! Forge capability contract and connector interface.

use std::collections::HashMap;
use std::fmt;
use std::process::Command;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const FORGE_CAPABILITY_VERSION: &str = "1.1.0";
pub const FORGE_CAPABILITY_CANONICAL_URL: &str = "https://raw.githubusercontent.com/tesserine/commons/6924159fc4ff58745f0e2c68ed16849ffd9b4086/schemas/forge-capability/v1/forge-capability.schema.json";

pub static FORGE_CAPABILITY_SCHEMA: LazyLock<Value> = LazyLock::new(connector_descriptor_schema);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Operation {
    ReadTicket,
    CreateTicket,
    ClaimWorkUnit,
    RecordProgress,
    ReflectDisposition,
    CloseOut,
    DeliverChangeProposal,
    ApplyApprovedChange,
}

impl Operation {
    pub const ALL: [Operation; 8] = [
        Operation::ReadTicket,
        Operation::CreateTicket,
        Operation::ClaimWorkUnit,
        Operation::RecordProgress,
        Operation::ReflectDisposition,
        Operation::CloseOut,
        Operation::DeliverChangeProposal,
        Operation::ApplyApprovedChange,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Operation::ReadTicket => "read-ticket",
            Operation::CreateTicket => "create-ticket",
            Operation::ClaimWorkUnit => "claim-work-unit",
            Operation::RecordProgress => "record-progress",
            Operation::ReflectDisposition => "reflect-disposition",
            Operation::CloseOut => "close-out",
            Operation::DeliverChangeProposal => "deliver-change-proposal",
            Operation::ApplyApprovedChange => "apply-approved-change",
        }
    }

    pub fn input_schema_ref(self) -> &'static str {
        match self {
            Operation::ReadTicket => "#/$defs/read-ticket-input",
            Operation::CreateTicket => "#/$defs/create-ticket-input",
            Operation::ClaimWorkUnit => "#/$defs/claim-work-unit-input",
            Operation::RecordProgress => "#/$defs/record-progress-input",
            Operation::ReflectDisposition => "#/$defs/reflect-disposition-input",
            Operation::CloseOut => "#/$defs/close-out-input",
            Operation::DeliverChangeProposal => "#/$defs/deliver-change-proposal-input",
            Operation::ApplyApprovedChange => "#/$defs/apply-approved-change-input",
        }
    }

    pub fn output_schema_ref(self) -> &'static str {
        match self {
            Operation::ReadTicket | Operation::CreateTicket => "#/$defs/ticket-snapshot",
            Operation::ClaimWorkUnit | Operation::RecordProgress => "#/$defs/work-unit-effect",
            Operation::ReflectDisposition => "#/$defs/disposition-effect",
            Operation::CloseOut => "#/$defs/close-out-effect",
            Operation::DeliverChangeProposal => "#/$defs/change-proposal",
            Operation::ApplyApprovedChange => "#/$defs/apply-result",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handle {
    pub id: String,
    pub display: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeToolDescriptor {
    pub operation: Operation,
    pub name: String,
    pub input_schema: String,
    pub output_schema: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeToolSetDescriptor {
    pub capability: String,
    pub version: String,
    pub handle_schema: String,
    pub tools: Vec<ForgeToolDescriptor>,
}

pub fn forge_operation_descriptors(_provider: &str) -> ForgeToolSetDescriptor {
    ForgeToolSetDescriptor {
        capability: "forge".to_string(),
        version: FORGE_CAPABILITY_VERSION.to_string(),
        handle_schema: "#/$defs/handle".to_string(),
        tools: Operation::ALL
            .iter()
            .copied()
            .map(|operation| ForgeToolDescriptor {
                operation,
                name: operation.name().to_string(),
                input_schema: operation.input_schema_ref().to_string(),
                output_schema: operation.output_schema_ref().to_string(),
            })
            .collect(),
    }
}

pub fn connector_descriptor_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "Forge Capability",
        "description": "Vendored schema for forge capability connector descriptors. Version 1.1.0. Canonical provenance is FORGE_CAPABILITY_CANONICAL_URL.",
        "type": "object",
        "required": ["capability", "version", "handle_schema", "tools"],
        "additionalProperties": false,
        "properties": {
            "capability": { "const": "forge" },
            "version": { "const": "1.1.0" },
            "handle_schema": { "const": "#/$defs/handle" },
            "tools": {
                "type": "array",
                "minItems": 8,
                "maxItems": 8,
                "items": { "$ref": "#/$defs/tool" }
            }
        },
        "$defs": capability_defs(),
    })
}

pub fn capability_defs() -> Value {
    json!({
        "operation-name": {
            "type": "string",
            "enum": Operation::ALL.iter().map(|operation| operation.name()).collect::<Vec<_>>()
        },
        "handle": handle_schema(),
        "tool": {
            "type": "object",
            "required": ["operation", "name", "input_schema", "output_schema"],
            "additionalProperties": false,
            "properties": {
                "operation": { "$ref": "#/$defs/operation-name" },
                "name": { "type": "string", "minLength": 1 },
                "input_schema": { "type": "string", "minLength": 1 },
                "output_schema": { "type": "string", "minLength": 1 }
            }
        },
        "read-ticket-input": {
            "type": "object",
            "required": ["reference"],
            "additionalProperties": false,
            "properties": { "reference": { "type": "string", "minLength": 1 } }
        },
        "create-ticket-input": {
            "type": "object",
            "required": ["title", "body"],
            "additionalProperties": false,
            "properties": {
                "title": { "type": "string", "minLength": 1 },
                "body": { "type": "string", "minLength": 1 }
            }
        },
        "claim-work-unit-input": handle_input_schema("handle"),
        "record-progress-input": {
            "type": "object",
            "required": ["handle", "body"],
            "additionalProperties": false,
            "properties": {
                "handle": { "$ref": "#/$defs/handle" },
                "body": { "type": "string", "minLength": 1 }
            }
        },
        "deliver-change-proposal-input": {
            "type": "object",
            "required": ["work_unit", "branch", "commit", "base", "summary", "body", "version"],
            "additionalProperties": false,
            "properties": {
                "work_unit": { "$ref": "#/$defs/handle" },
                "branch": { "type": "string", "minLength": 1 },
                "commit": { "type": "string", "minLength": 1 },
                "base": { "type": "string", "minLength": 1 },
                "summary": { "type": "string", "minLength": 1 },
                "body": { "type": "string", "minLength": 1 },
                "version": { "type": "integer", "minimum": 1 }
            }
        },
        "reflect-disposition-input": {
            "type": "object",
            "required": ["work_unit", "change", "disposition", "body"],
            "additionalProperties": false,
            "properties": {
                "work_unit": { "$ref": "#/$defs/handle" },
                "change": { "$ref": "#/$defs/handle" },
                "disposition": { "type": "object" },
                "body": { "type": "string", "minLength": 1 }
            }
        },
        "apply-approved-change-input": {
            "type": "object",
            "required": ["work_unit", "change", "approved_version", "approved_commit", "base"],
            "additionalProperties": false,
            "properties": {
                "work_unit": { "$ref": "#/$defs/handle" },
                "change": { "$ref": "#/$defs/handle" },
                "approved_version": { "type": "integer", "minimum": 1 },
                "approved_commit": { "type": "string", "minLength": 1 },
                "base": { "type": "string", "minLength": 1 }
            }
        },
        "close-out-input": {
            "type": "object",
            "required": ["work_unit", "completion", "body"],
            "additionalProperties": false,
            "properties": {
                "work_unit": { "$ref": "#/$defs/handle" },
                "completion": { "type": "object" },
                "body": { "type": "string", "minLength": 1 }
            }
        },
        "ticket-snapshot": ticket_snapshot_schema(),
        "work-unit-effect": {
            "type": "object",
            "required": ["handle", "receipt"],
            "additionalProperties": false,
            "properties": {
                "handle": { "$ref": "#/$defs/handle" },
                "receipt": { "type": "string", "minLength": 1 }
            }
        },
        "change-proposal": {
            "type": "object",
            "required": ["handle", "work_unit", "commit", "version"],
            "additionalProperties": false,
            "properties": {
                "handle": { "$ref": "#/$defs/handle" },
                "work_unit": { "$ref": "#/$defs/handle" },
                "commit": { "type": "string", "minLength": 1 },
                "version": { "type": "integer", "minimum": 1 }
            }
        },
        "disposition-effect": two_handle_effect_schema(),
        "apply-result": {
            "type": "object",
            "required": ["work_unit", "change", "applied_commit", "receipt"],
            "additionalProperties": false,
            "properties": {
                "work_unit": { "$ref": "#/$defs/handle" },
                "change": { "$ref": "#/$defs/handle" },
                "applied_commit": { "type": "string", "minLength": 1 },
                "receipt": { "type": "string", "minLength": 1 }
            }
        },
        "close-out-effect": {
            "type": "object",
            "required": ["work_unit", "receipt"],
            "additionalProperties": false,
            "properties": {
                "work_unit": { "$ref": "#/$defs/handle" },
                "receipt": { "type": "string", "minLength": 1 }
            }
        }
    })
}

pub fn input_schema(operation: Operation) -> Value {
    definition(operation.input_schema_ref())
}

pub fn output_schema(operation: Operation) -> Value {
    definition(operation.output_schema_ref())
}

fn definition(reference: &str) -> Value {
    let key = reference
        .strip_prefix("#/$defs/")
        .expect("forge schema references should target $defs");
    capability_defs()[key].clone()
}

fn handle_schema() -> Value {
    json!({
        "type": "object",
        "required": ["id", "display"],
        "additionalProperties": false,
        "properties": {
            "id": { "type": "string", "minLength": 1 },
            "display": { "type": "string", "minLength": 1 }
        }
    })
}

fn handle_input_schema(field: &str) -> Value {
    json!({
        "type": "object",
        "required": [field],
        "additionalProperties": false,
        "properties": { field: { "$ref": "#/$defs/handle" } }
    })
}

fn ticket_snapshot_schema() -> Value {
    json!({
        "type": "object",
        "required": ["handle", "title", "state"],
        "additionalProperties": false,
        "properties": {
            "handle": { "$ref": "#/$defs/handle" },
            "title": { "type": "string", "minLength": 1 },
            "body": { "type": ["string", "null"] },
            "state": { "type": "string", "minLength": 1 }
        }
    })
}

fn two_handle_effect_schema() -> Value {
    json!({
        "type": "object",
        "required": ["work_unit", "change", "receipt"],
        "additionalProperties": false,
        "properties": {
            "work_unit": { "$ref": "#/$defs/handle" },
            "change": { "$ref": "#/$defs/handle" },
            "receipt": { "type": "string", "minLength": 1 }
        }
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialSource {
    None,
    Env(String),
    Command(Vec<String>),
}

impl CredentialSource {
    pub fn resolve(&self) -> Result<Option<String>, ForgeError> {
        match self {
            CredentialSource::None => Ok(None),
            CredentialSource::Env(name) => std::env::var(name)
                .map(Some)
                .map_err(|_| ForgeError::Config(format!("credential env var '{name}' is not set"))),
            CredentialSource::Command(argv) => {
                let (program, args) = argv
                    .split_first()
                    .ok_or_else(|| ForgeError::Config("credential command is empty".to_string()))?;
                let output = Command::new(program).args(args).output().map_err(|error| {
                    ForgeError::Config(format!("credential command failed: {error}"))
                })?;
                if !output.status.success() {
                    return Err(ForgeError::Config(format!(
                        "credential command exited with {}",
                        output.status
                    )));
                }
                let secret = String::from_utf8(output.stdout).map_err(|_| {
                    ForgeError::Config("credential command output is not UTF-8".to_string())
                })?;
                Ok(Some(secret.trim_end_matches(['\r', '\n']).to_string()))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorConfig {
    pub provider: String,
    pub owner: Option<String>,
    pub name: Option<String>,
    pub tracker_id: Option<u64>,
    pub endpoint: Option<String>,
    pub credential: CredentialSource,
    pub extra: HashMap<String, String>,
}

impl ConnectorConfig {
    pub fn github(owner: &str, name: &str, credential: CredentialSource) -> Self {
        Self {
            provider: "github".to_string(),
            owner: Some(owner.to_string()),
            name: Some(name.to_string()),
            tracker_id: None,
            endpoint: None,
            credential,
            extra: HashMap::new(),
        }
    }

    pub fn sourcehut(
        owner: &str,
        name: &str,
        tracker_id: u64,
        endpoint: &str,
        credential: CredentialSource,
    ) -> Self {
        Self {
            provider: "sourcehut".to_string(),
            owner: Some(owner.to_string()),
            name: Some(name.to_string()),
            tracker_id: Some(tracker_id),
            endpoint: Some(endpoint.to_string()),
            credential,
            extra: HashMap::new(),
        }
    }

    pub fn required_owner(&self) -> Result<&str, ForgeError> {
        self.owner
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ForgeError::Config("connector owner is required".to_string()))
    }

    pub fn required_name(&self) -> Result<&str, ForgeError> {
        self.name
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ForgeError::Config("connector name is required".to_string()))
    }

    pub fn required_tracker_id(&self) -> Result<u64, ForgeError> {
        self.tracker_id
            .ok_or_else(|| ForgeError::Config("connector tracker_id is required".to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForgeError {
    Config(String),
    InvalidInput(String),
    ForeignScope(String),
    UnsupportedOperation(Operation),
    Transport(String),
}

impl fmt::Display for ForgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ForgeError::Config(message) => write!(f, "{message}"),
            ForgeError::InvalidInput(message) => write!(f, "{message}"),
            ForgeError::ForeignScope(message) => write!(f, "foreign scope: {message}"),
            ForgeError::UnsupportedOperation(operation) => {
                write!(f, "unsupported operation '{}'", operation.name())
            }
            ForgeError::Transport(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ForgeError {}

pub trait ForgeConnector: Send + Sync {
    fn provider(&self) -> &'static str;
    fn descriptor(&self) -> ForgeToolSetDescriptor {
        forge_operation_descriptors(self.provider())
    }
    fn dry_run(&self, operation: Operation, input: Value) -> Result<Value, ForgeError>;
}

#[macro_export]
macro_rules! json_args {
    ($($json:tt)+) => {
        serde_json::json!($($json)+)
    };
}

pub fn require_string<'a>(input: &'a Value, key: &str) -> Result<&'a str, ForgeError> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ForgeError::InvalidInput(format!("'{key}' is required")))
}

pub fn handle_value(handle: Handle) -> Value {
    json!({ "id": handle.id, "display": handle.display })
}
