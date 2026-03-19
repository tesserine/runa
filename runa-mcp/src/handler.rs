use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rmcp::Error as McpError;
use rmcp::ServerHandler;
use rmcp::model::*;
use rmcp::service::{RequestContext, RoleServer};
use serde_json::Value;

use libagent::context::build_context;
use libagent::validation::validate_artifact;
use libagent::{ArtifactStore, ArtifactType, Manifest, ProtocolDeclaration};

use crate::context::render_context_prompt;

pub struct RunaHandler {
    protocol: ProtocolDeclaration,
    work_unit: Option<String>,
    state: Mutex<HandlerState>,
    workspace_dir: PathBuf,
    #[allow(dead_code)]
    manifest: Manifest,
    tools: Vec<Tool>,
    /// Maps artifact type name → full JSON Schema (with work_unit intact).
    tool_schemas: HashMap<String, Value>,
}

struct HandlerState {
    store: ArtifactStore,
    capstone_produced: bool,
}

impl RunaHandler {
    pub fn new(
        protocol: ProtocolDeclaration,
        work_unit: Option<String>,
        store: ArtifactStore,
        manifest: Manifest,
        workspace_dir: PathBuf,
    ) -> Self {
        let mut tools = Vec::new();
        let mut tool_schemas = HashMap::new();

        let output_types: Vec<&String> = protocol
            .produces
            .iter()
            .chain(protocol.may_produce.iter())
            .collect();

        for type_name in output_types {
            if let Some(at) = store.artifact_type(type_name) {
                let stripped = strip_work_unit(&at.schema);
                let schema_obj = match stripped {
                    Value::Object(map) => map,
                    _ => serde_json::Map::new(),
                };

                tools.push(Tool::new(
                    type_name.clone(),
                    format!("Produce a {type_name} artifact"),
                    Arc::new(schema_obj),
                ));
                tool_schemas.insert(type_name.clone(), at.schema.clone());
            }
        }

        Self {
            protocol,
            work_unit,
            state: Mutex::new(HandlerState {
                store,
                capstone_produced: false,
            }),
            workspace_dir,
            manifest,
            tools,
            tool_schemas,
        }
    }
}

/// Remove `work_unit` from a JSON Schema's `properties` and `required`.
fn strip_work_unit(schema: &Value) -> Value {
    let mut schema = schema.clone();
    if let Value::Object(ref mut map) = schema {
        if let Some(Value::Object(props)) = map.get_mut("properties") {
            props.remove("work_unit");
        }
        if let Some(Value::Array(required)) = map.get_mut("required") {
            required.retain(|v| v.as_str() != Some("work_unit"));
        }
    }
    schema
}

/// Reject instance IDs that would cause path traversal or ambiguity.
fn validate_instance_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("instance ID must not be empty".into());
    }
    if id.contains('/') || id.contains('\\') {
        return Err("instance ID must not contain path separators".into());
    }
    if id.contains("..") {
        return Err("instance ID must not contain path traversal sequences".into());
    }
    Ok(())
}

impl ServerHandler for RunaHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::default(),
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
            server_info: Implementation {
                name: "runa-mcp".into(),
                version: libagent::version().into(),
            },
            instructions: Some(format!(
                "MCP server for protocol '{}'. Use the 'context' prompt to see inputs and expected outputs.",
                self.protocol.name
            )),
        }
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult {
            next_cursor: None,
            tools: self.tools.clone(),
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let tool_name = request.name.as_ref();

        // Look up the full schema for this artifact type.
        let full_schema = self
            .tool_schemas
            .get(tool_name)
            .ok_or_else(|| McpError::invalid_params(format!("unknown tool: {tool_name}"), None))?;

        // Build the artifact data from arguments.
        let mut data = match request.arguments {
            Some(args) => Value::Object(args),
            None => Value::Object(serde_json::Map::new()),
        };

        // Determine instance_id: work_unit when scoped, type name when unscoped.
        let instance_id = match &self.work_unit {
            Some(wu) => wu.clone(),
            None => tool_name.to_string(),
        };

        validate_instance_id(&instance_id).map_err(|e| McpError::invalid_params(e, None))?;

        // Inject work_unit into data if the schema declares it.
        if let Value::Object(schema_map) = full_schema
            && let Some(Value::Object(props)) = schema_map.get("properties")
            && props.contains_key("work_unit")
            && let (Value::Object(data_map), Some(wu)) = (&mut data, &self.work_unit)
        {
            data_map.insert("work_unit".to_string(), Value::String(wu.clone()));
        }

        // Validate against the full schema (including work_unit).
        let at = ArtifactType {
            name: tool_name.to_string(),
            schema: full_schema.clone(),
        };
        if let Err(e) = validate_artifact(&data, &at) {
            let msg = match e {
                libagent::ValidationError::InvalidArtifact { violations, .. } => violations
                    .iter()
                    .map(|v| format!("{}: {}", v.instance_path, v.description))
                    .collect::<Vec<_>>()
                    .join("\n"),
                libagent::ValidationError::InvalidSchema { detail, .. } => {
                    format!("schema error: {detail}")
                }
            };
            return Ok(CallToolResult::error(vec![Content::text(msg)]));
        }

        // Write artifact to workspace.
        let type_dir = self.workspace_dir.join(tool_name);
        std::fs::create_dir_all(&type_dir).map_err(|e| {
            McpError::internal_error(format!("failed to create directory: {e}"), None)
        })?;
        let artifact_path = type_dir.join(format!("{instance_id}.json"));
        let json = serde_json::to_string_pretty(&data)
            .map_err(|e| McpError::internal_error(format!("serialization error: {e}"), None))?;
        std::fs::write(&artifact_path, &json).map_err(|e| {
            McpError::internal_error(format!("failed to write artifact: {e}"), None)
        })?;

        // Record in store.
        let mut state = self.state.lock().unwrap();
        state
            .store
            .record(tool_name, &instance_id, &artifact_path, &data)
            .map_err(|e| McpError::internal_error(format!("store error: {e}"), None))?;

        // Track capstone: set flag if this is a `produces` type.
        if self.protocol.produces.iter().any(|t| t == tool_name) {
            state.capstone_produced = true;
        }

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Produced {tool_name}/{instance_id}.json"
        ))]))
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        Ok(ListPromptsResult {
            next_cursor: None,
            prompts: vec![Prompt::new(
                "context",
                Some("Protocol context and instructions"),
                None::<Vec<PromptArgument>>,
            )],
        })
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, McpError> {
        if request.name != "context" {
            return Err(McpError::invalid_params(
                format!("unknown prompt: {}", request.name),
                None,
            ));
        }

        let state = self.state.lock().unwrap();
        let injection = build_context(&self.protocol, &state.store, self.work_unit.as_deref());
        Ok(render_context_prompt(
            &injection,
            &state.store,
            &self.workspace_dir,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libagent::model::TriggerCondition;
    use serde_json::json;

    #[test]
    fn strip_work_unit_removes_from_properties_and_required() {
        let schema = json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "work_unit": { "type": "string" }
            },
            "required": ["title", "work_unit"]
        });

        let stripped = strip_work_unit(&schema);
        let props = stripped["properties"].as_object().unwrap();
        assert!(props.contains_key("title"));
        assert!(!props.contains_key("work_unit"));

        let required: Vec<&str> = stripped["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(required, vec!["title"]);
    }

    #[test]
    fn strip_work_unit_noop_when_absent() {
        let schema = json!({
            "type": "object",
            "properties": { "title": { "type": "string" } },
            "required": ["title"]
        });
        let stripped = strip_work_unit(&schema);
        assert_eq!(schema, stripped);
    }

    #[test]
    fn validate_instance_id_rejects_separators() {
        assert!(validate_instance_id("good-name").is_ok());
        assert!(validate_instance_id("path/traversal").is_err());
        assert!(validate_instance_id("path\\traversal").is_err());
        assert!(validate_instance_id("..").is_err());
        assert!(validate_instance_id("").is_err());
    }

    #[test]
    fn handler_derives_tools_from_output_types() {
        let tmp = tempfile::tempdir().unwrap();
        let types = vec![
            ArtifactType {
                name: "constraints".into(),
                schema: json!({ "type": "object" }),
            },
            ArtifactType {
                name: "implementation".into(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string" },
                        "work_unit": { "type": "string" }
                    }
                }),
            },
        ];
        let store = ArtifactStore::new(types.clone(), tmp.path().join("store")).unwrap();
        let manifest = libagent::Manifest {
            name: "test".into(),
            artifact_types: types,
            protocols: Vec::new(),
        };
        let protocol = ProtocolDeclaration {
            name: "implement".into(),
            requires: vec!["constraints".into()],
            accepts: Vec::new(),
            produces: vec!["implementation".into()],
            may_produce: Vec::new(),
            trigger: TriggerCondition::OnArtifact {
                name: "constraints".into(),
            },
        };

        let handler = RunaHandler::new(
            protocol,
            Some("wu-1".into()),
            store,
            manifest,
            tmp.path().join("workspace"),
        );

        // Only output types become tools, not requires.
        assert_eq!(handler.tools.len(), 1);
        assert_eq!(handler.tools[0].name.as_ref(), "implementation");

        // The tool schema should not have work_unit in properties.
        let tool_props = handler.tools[0]
            .input_schema
            .get("properties")
            .and_then(|v| v.as_object());
        if let Some(props) = tool_props {
            assert!(!props.contains_key("work_unit"));
        }
    }
}
