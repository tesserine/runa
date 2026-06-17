use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const FORGE_CAPABILITY_VERSION: &str = "1.1.0";
pub const FORGE_CAPABILITY_COMMONS_COMMIT: &str = "6924159fc4ff58745f0e2c68ed16849ffd9b4086";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
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
}

impl fmt::Display for ForgeOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handle {
    pub id: String,
    pub display: String,
}

impl Handle {
    pub fn new(id: impl Into<String>, display: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            display: display.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ForgeTool {
    pub operation: ForgeOperation,
    pub name: String,
    pub description: String,
    pub input_schema: Map<String, Value>,
    pub output_schema: Map<String, Value>,
    pub representative_input: Value,
    pub representative_output: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ForgeToolSet {
    pub provider: String,
    pub tools: Vec<ForgeTool>,
}

impl ForgeToolSet {
    pub fn new(provider: impl Into<String>, tools: Vec<ForgeTool>) -> Self {
        Self {
            provider: provider.into(),
            tools,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeConnectorsConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forge: Vec<ForgeConnectorConfig>,
}

impl ForgeConnectorsConfig {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeConnectorConfig {
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias_prefix: Option<String>,
    pub owner: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracker_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_id: Option<u64>,
    #[serde(default, skip_serializing_if = "ForgeCredentialConfig::is_default")]
    pub credentials: ForgeCredentialConfig,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeCredentialConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
}

impl ForgeCredentialConfig {
    fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolAliases {
    pub aliases: BTreeMap<(String, String), String>,
}

impl ToolAliases {
    pub fn insert(
        &mut self,
        provider: impl Into<String>,
        tool_name: impl Into<String>,
        alias: impl Into<String>,
    ) {
        self.aliases
            .insert((provider.into(), tool_name.into()), alias.into());
    }

    fn alias_for(&self, provider: &str, tool_name: &str) -> Option<&str> {
        self.aliases
            .get(&(provider.to_owned(), tool_name.to_owned()))
            .map(String::as_str)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposeError {
    Collision {
        tool_name: String,
        providers: Vec<String>,
    },
    AliasCollision {
        tool_name: String,
    },
}

impl fmt::Display for ComposeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ComposeError::Collision {
                tool_name,
                providers,
            } => write!(
                f,
                "forge tool collision for {tool_name} across providers: {}",
                providers.join(", ")
            ),
            ComposeError::AliasCollision { tool_name } => {
                write!(f, "forge tool alias collision for {tool_name}")
            }
        }
    }
}

impl std::error::Error for ComposeError {}

pub fn compose_tool_sets(
    sets: Vec<ForgeToolSet>,
    aliases: ToolAliases,
) -> Result<Vec<ForgeTool>, ComposeError> {
    let mut natural_owners: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut natural_order = Vec::new();
    for set in &sets {
        for tool in &set.tools {
            if aliases.alias_for(&set.provider, &tool.name).is_none() {
                let owners = natural_owners.entry(tool.name.clone()).or_default();
                if owners.is_empty() {
                    natural_order.push(tool.name.clone());
                }
                owners.push(set.provider.clone());
            }
        }
    }

    for tool_name in natural_order {
        let providers = natural_owners
            .remove(&tool_name)
            .expect("ordered tool name must exist in owner map");
        if providers.len() > 1 {
            return Err(ComposeError::Collision {
                tool_name,
                providers,
            });
        }
    }

    let mut seen = BTreeSet::new();
    let mut composed = Vec::new();
    for set in sets {
        for mut tool in set.tools {
            if let Some(alias) = aliases.alias_for(&set.provider, &tool.name) {
                tool.name = alias.to_owned();
            }
            if !seen.insert(tool.name.clone()) {
                return Err(ComposeError::AliasCollision {
                    tool_name: tool.name,
                });
            }
            composed.push(tool);
        }
    }
    Ok(composed)
}

pub fn forge_tool_set(provider: impl Into<String>) -> ForgeToolSet {
    ForgeToolSet::new(
        provider,
        ForgeOperation::ALL
            .iter()
            .copied()
            .map(forge_tool)
            .collect(),
    )
}

pub fn forge_tool(operation: ForgeOperation) -> ForgeTool {
    ForgeTool {
        operation,
        name: operation.name().to_owned(),
        description: format!("Forge capability operation `{}`.", operation.name()),
        input_schema: schema_object(operation, SchemaSide::Input),
        output_schema: schema_object(operation, SchemaSide::Output),
        representative_input: representative_input(operation),
        representative_output: representative_output(operation),
    }
}

#[derive(Clone, Copy)]
enum SchemaSide {
    Input,
    Output,
}

fn schema_object(operation: ForgeOperation, side: SchemaSide) -> Map<String, Value> {
    let mut root = match (operation, side) {
        (ForgeOperation::ReadTicket, SchemaSide::Input) => object_schema(
            &["reference"],
            vec![("reference", json!({ "type": "string", "minLength": 1 }))],
        ),
        (ForgeOperation::ReadTicket, SchemaSide::Output) => ticket_snapshot_schema(),
        (ForgeOperation::CreateTicket, SchemaSide::Input) => object_schema(
            &["title", "body"],
            vec![
                ("title", json!({ "type": "string", "minLength": 1 })),
                ("body", json!({ "type": "string" })),
                (
                    "labels",
                    json!({ "type": "array", "items": { "type": "string" } }),
                ),
            ],
        ),
        (ForgeOperation::CreateTicket, SchemaSide::Output) => ticket_snapshot_schema(),
        (ForgeOperation::ClaimWorkUnit, SchemaSide::Input) => object_schema(
            &["work_unit"],
            vec![
                ("work_unit", json!({ "$ref": "#/$defs/handle" })),
                ("claimant", json!({ "type": "string", "minLength": 1 })),
            ],
        ),
        (ForgeOperation::ClaimWorkUnit, SchemaSide::Output) => effect_schema("claimed"),
        (ForgeOperation::RecordProgress, SchemaSide::Input) => object_schema(
            &["work_unit", "body"],
            vec![
                ("work_unit", json!({ "$ref": "#/$defs/handle" })),
                ("body", json!({ "type": "string", "minLength": 1 })),
            ],
        ),
        (ForgeOperation::RecordProgress, SchemaSide::Output) => effect_schema("progress-recorded"),
        (ForgeOperation::DeliverChangeProposal, SchemaSide::Input) => object_schema(
            &[
                "work_unit",
                "branch",
                "commit",
                "base",
                "summary",
                "body",
                "version",
            ],
            vec![
                ("work_unit", json!({ "$ref": "#/$defs/handle" })),
                ("branch", json!({ "type": "string", "minLength": 1 })),
                ("commit", json!({ "type": "string", "minLength": 1 })),
                ("base", json!({ "type": "string", "minLength": 1 })),
                ("summary", json!({ "type": "string", "minLength": 1 })),
                ("body", json!({ "type": "string" })),
                ("version", json!({ "type": "string", "minLength": 1 })),
            ],
        ),
        (ForgeOperation::DeliverChangeProposal, SchemaSide::Output) => object_schema(
            &["change", "work_unit", "commit", "version"],
            vec![
                ("change", json!({ "$ref": "#/$defs/handle" })),
                ("work_unit", json!({ "$ref": "#/$defs/handle" })),
                ("commit", json!({ "type": "string", "minLength": 1 })),
                ("version", json!({ "type": "string", "minLength": 1 })),
                ("url", json!({ "type": "string", "minLength": 1 })),
            ],
        ),
        (ForgeOperation::ReflectDisposition, SchemaSide::Input) => object_schema(
            &["work_unit", "change", "disposition", "body"],
            vec![
                ("work_unit", json!({ "$ref": "#/$defs/handle" })),
                ("change", json!({ "$ref": "#/$defs/handle" })),
                ("disposition", json!({ "$ref": "#/$defs/disposition" })),
                ("body", json!({ "type": "string" })),
            ],
        ),
        (ForgeOperation::ReflectDisposition, SchemaSide::Output) => {
            effect_schema("disposition-reflected")
        }
        (ForgeOperation::ApplyApprovedChange, SchemaSide::Input) => object_schema(
            &[
                "work_unit",
                "change",
                "approved_version",
                "approved_commit",
                "base",
            ],
            vec![
                ("work_unit", json!({ "$ref": "#/$defs/handle" })),
                ("change", json!({ "$ref": "#/$defs/handle" })),
                (
                    "approved_version",
                    json!({ "type": "string", "minLength": 1 }),
                ),
                (
                    "approved_commit",
                    json!({ "type": "string", "minLength": 1 }),
                ),
                ("base", json!({ "type": "string", "minLength": 1 })),
            ],
        ),
        (ForgeOperation::ApplyApprovedChange, SchemaSide::Output) => object_schema(
            &["work_unit", "change", "applied_commit"],
            vec![
                ("work_unit", json!({ "$ref": "#/$defs/handle" })),
                ("change", json!({ "$ref": "#/$defs/handle" })),
                (
                    "applied_commit",
                    json!({ "type": "string", "minLength": 1 }),
                ),
                ("status", json!({ "const": "applied" })),
            ],
        ),
        (ForgeOperation::CloseOut, SchemaSide::Input) => object_schema(
            &["work_unit", "completion", "body"],
            vec![
                ("work_unit", json!({ "$ref": "#/$defs/handle" })),
                ("completion", json!({ "$ref": "#/$defs/completion" })),
                ("body", json!({ "type": "string" })),
            ],
        ),
        (ForgeOperation::CloseOut, SchemaSide::Output) => effect_schema("closed-out"),
    };

    root.insert(
        "$schema".to_owned(),
        json!("https://json-schema.org/draft/2020-12/schema"),
    );
    root.insert("$defs".to_owned(), defs());
    root
}

fn object_schema(required: &[&str], fields: Vec<(&str, Value)>) -> Map<String, Value> {
    let mut properties = Map::new();
    for (name, schema) in fields {
        properties.insert(name.to_owned(), schema);
    }

    let mut root = Map::new();
    root.insert("type".to_owned(), json!("object"));
    root.insert("additionalProperties".to_owned(), json!(false));
    root.insert(
        "required".to_owned(),
        Value::Array(required.iter().map(|field| json!(field)).collect()),
    );
    root.insert("properties".to_owned(), Value::Object(properties));
    root
}

fn ticket_snapshot_schema() -> Map<String, Value> {
    object_schema(
        &["handle", "title", "body", "state"],
        vec![
            ("handle", json!({ "$ref": "#/$defs/handle" })),
            ("title", json!({ "type": "string" })),
            ("body", json!({ "type": "string" })),
            ("state", json!({ "enum": ["open", "closed"] })),
            ("url", json!({ "type": "string" })),
        ],
    )
}

fn effect_schema(status: &'static str) -> Map<String, Value> {
    object_schema(
        &["work_unit", "status"],
        vec![
            ("work_unit", json!({ "$ref": "#/$defs/handle" })),
            ("status", json!({ "const": status })),
            ("url", json!({ "type": "string" })),
        ],
    )
}

fn defs() -> Value {
    json!({
        "handle": {
            "type": "object",
            "additionalProperties": false,
            "required": ["id", "display"],
            "properties": {
                "id": { "type": "string", "minLength": 1 },
                "display": { "type": "string", "minLength": 1 }
            }
        },
        "disposition": {
            "enum": ["approved", "needs-revision", "rejected"]
        },
        "completion": {
            "enum": ["complete", "not-planned", "duplicate"]
        }
    })
}

fn example_work_unit() -> Value {
    json!({
        "id": "example:tracker:ticket:203",
        "display": "example#203"
    })
}

fn example_change() -> Value {
    json!({
        "id": "example:change:1",
        "display": "example!1"
    })
}

fn representative_input(operation: ForgeOperation) -> Value {
    match operation {
        ForgeOperation::ReadTicket => json!({ "reference": "203" }),
        ForgeOperation::CreateTicket => json!({
            "title": "Connector layer",
            "body": "Build forge connector surface.",
            "labels": ["task"]
        }),
        ForgeOperation::ClaimWorkUnit => json!({
            "work_unit": example_work_unit(),
            "claimant": "core"
        }),
        ForgeOperation::RecordProgress => json!({
            "work_unit": example_work_unit(),
            "body": "Progress recorded."
        }),
        ForgeOperation::DeliverChangeProposal => json!({
            "work_unit": example_work_unit(),
            "branch": "work/203-connectors",
            "commit": "abc123",
            "base": "main",
            "summary": "Implement connector layer",
            "body": "Change proposal body.",
            "version": "1"
        }),
        ForgeOperation::ReflectDisposition => json!({
            "work_unit": example_work_unit(),
            "change": example_change(),
            "disposition": "approved",
            "body": "Approved."
        }),
        ForgeOperation::ApplyApprovedChange => json!({
            "work_unit": example_work_unit(),
            "change": example_change(),
            "approved_version": "1",
            "approved_commit": "abc123",
            "base": "main"
        }),
        ForgeOperation::CloseOut => json!({
            "work_unit": example_work_unit(),
            "completion": "complete",
            "body": "Closed."
        }),
    }
}

fn representative_output(operation: ForgeOperation) -> Value {
    match operation {
        ForgeOperation::ReadTicket | ForgeOperation::CreateTicket => json!({
            "handle": example_work_unit(),
            "title": "Connector layer",
            "body": "Build forge connector surface.",
            "state": "open",
            "url": "https://example.invalid/ticket/203"
        }),
        ForgeOperation::ClaimWorkUnit => json!({
            "work_unit": example_work_unit(),
            "status": "claimed",
            "url": "https://example.invalid/ticket/203"
        }),
        ForgeOperation::RecordProgress => json!({
            "work_unit": example_work_unit(),
            "status": "progress-recorded",
            "url": "https://example.invalid/ticket/203"
        }),
        ForgeOperation::DeliverChangeProposal => json!({
            "change": example_change(),
            "work_unit": example_work_unit(),
            "commit": "abc123",
            "version": "1",
            "url": "https://example.invalid/change/1"
        }),
        ForgeOperation::ReflectDisposition => json!({
            "work_unit": example_work_unit(),
            "status": "disposition-reflected",
            "url": "https://example.invalid/ticket/203"
        }),
        ForgeOperation::ApplyApprovedChange => json!({
            "work_unit": example_work_unit(),
            "change": example_change(),
            "applied_commit": "abc123",
            "status": "applied"
        }),
        ForgeOperation::CloseOut => json!({
            "work_unit": example_work_unit(),
            "status": "closed-out",
            "url": "https://example.invalid/ticket/203"
        }),
    }
}
