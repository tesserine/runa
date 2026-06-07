use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rmcp::Error as McpError;
use rmcp::ServerHandler;
use rmcp::model::*;
use rmcp::service::{RequestContext, RoleServer};
use serde_json::Value;
use tracing::{info, warn};

use libagent::context::ContextInjectionView;
use libagent::validation::validate_artifact;
use libagent::{
    ArtifactStore, ArtifactType, ProtocolDeclaration, SessionState, validate_output_scope,
};

const DRIVER_TOOL_NAMES: [&str; 3] = ["readiness", "next-protocol-context", "advance"];

pub struct RunaHandler {
    protocol: Option<ProtocolDeclaration>,
    work_unit: Option<String>,
    state: Option<Mutex<HandlerState>>,
    workspace_dir: PathBuf,
    tools: Vec<Tool>,
    /// Maps artifact type name → full JSON Schema (with work_unit intact).
    tool_schemas: HashMap<String, Value>,
    session: Option<Mutex<SessionState>>,
}

struct HandlerState {
    store: ArtifactStore,
}

impl RunaHandler {
    pub fn new(
        protocol: ProtocolDeclaration,
        work_unit: Option<String>,
        store: ArtifactStore,
        workspace_dir: PathBuf,
    ) -> Self {
        let (tools, tool_schemas) =
            output_tools_for_protocol(&protocol, work_unit.as_deref(), &store, false);

        Self {
            protocol: Some(protocol),
            work_unit,
            state: Some(Mutex::new(HandlerState { store })),
            workspace_dir,
            tools,
            tool_schemas,
            session: None,
        }
    }

    pub fn new_session(
        working_dir: PathBuf,
        config_override: Option<&Path>,
        work_unit: Option<String>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let session = SessionState::open(working_dir, config_override, work_unit)?;
        validate_session_current_step(&session)?;

        Ok(Self {
            protocol: None,
            work_unit: session
                .current_step()
                .and_then(|step| step.work_unit.clone()),
            state: None,
            workspace_dir: session.workspace_dir().to_path_buf(),
            tools: Vec::new(),
            tool_schemas: HashMap::new(),
            session: Some(Mutex::new(session)),
        })
    }

    async fn call_session_tool(
        &self,
        session: &Mutex<SessionState>,
        tool_name: &str,
        request: CallToolRequestParam,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        match tool_name {
            "readiness" => {
                let mut session = session.lock().unwrap();
                let result = session
                    .readiness()
                    .map_err(|error| McpError::internal_error(error.to_string(), None))?;
                return json_tool_result(&result);
            }
            "next-protocol-context" => {
                let mut session = session.lock().unwrap();
                let (context, rendered_prompt) = session
                    .next_context()
                    .map_err(|error| McpError::internal_error(error.to_string(), None))?;
                let payload = serde_json::json!({
                    "version": 1,
                    "context": ContextInjectionView::from(&context),
                    "rendered_prompt": rendered_prompt,
                });
                return json_tool_result(&payload);
            }
            "advance" => {
                let (result, tool_list_changed) = {
                    let mut session = session.lock().unwrap();
                    let before_step = session.current_step().cloned();
                    let outcome = session
                        .advance_with_validator(|next_protocol, store| {
                            if let Some(protocol) = next_protocol {
                                for type_name in protocol
                                    .produces
                                    .iter()
                                    .chain(protocol.required_choice_members())
                                {
                                    if DRIVER_TOOL_NAMES.contains(&type_name.as_str()) {
                                        return Err(format!(
                                            "required output type '{type_name}' collides with reserved session driver verb"
                                        ));
                                    }
                                }
                                validate_output_types(protocol, store, Some(""))
                            } else {
                                Ok(())
                            }
                        })
                        .map_err(|error| McpError::internal_error(error.to_string(), None))?;
                    let after_step = outcome.next_step.clone();
                    (json_tool_result(&outcome)?, before_step != after_step)
                };
                if tool_list_changed {
                    context
                        .peer
                        .notify_tool_list_changed()
                        .await
                        .map_err(|error| McpError::internal_error(error.to_string(), None))?;
                }
                return Ok(result);
            }
            _ => {}
        }

        let mut session = session.lock().unwrap();
        let current_step = session
            .current_step()
            .cloned()
            .ok_or_else(|| McpError::invalid_params("session has no current step", None))?;
        let protocol_name = current_step.protocol.clone();
        append_tool_event(
            "tool_call",
            &protocol_name,
            current_step.work_unit.as_deref(),
            tool_name,
            request
                .arguments
                .as_ref()
                .map(|arguments| Value::Object(arguments.clone())),
            None,
        )?;

        let (_, schemas) = session_tools_and_schemas(&session)
            .map_err(|error| McpError::internal_error(error, None))?;
        let full_schema = schemas
            .get(tool_name)
            .ok_or_else(|| McpError::invalid_params(format!("unknown tool: {tool_name}"), None))?;
        let mut data = match request.arguments {
            Some(args) => Value::Object(args),
            None => Value::Object(serde_json::Map::new()),
        };

        let instance_id = extract_instance_id(&mut data)?;
        validate_instance_id(&instance_id).map_err(|e| McpError::invalid_params(e, None))?;

        let at = ArtifactType {
            name: tool_name.to_string(),
            schema: full_schema.clone(),
        };
        if at.schema_mentions_work_unit()
            && let (Value::Object(data_map), Some(wu)) =
                (&mut data, current_step.work_unit.as_ref())
        {
            data_map.insert("work_unit".to_string(), Value::String(wu.clone()));
        }

        if let Err(e) = validate_artifact(&data, &at) {
            let msg = validation_message(e);
            append_tool_event(
                "tool_result",
                &protocol_name,
                current_step.work_unit.as_deref(),
                tool_name,
                None,
                Some(&msg),
            )?;
            return Ok(CallToolResult::error(vec![Content::text(msg)]));
        }

        let type_dir = session.workspace_dir().join(tool_name);
        std::fs::create_dir_all(&type_dir).map_err(|e| {
            McpError::internal_error(format!("failed to create directory: {e}"), None)
        })?;
        let artifact_path = type_dir.join(format!("{instance_id}.json"));
        let json = serde_json::to_string_pretty(&data)
            .map_err(|e| McpError::internal_error(format!("serialization error: {e}"), None))?;
        std::fs::write(&artifact_path, &json).map_err(|e| {
            McpError::internal_error(format!("failed to write artifact: {e}"), None)
        })?;
        session
            .store_mut()
            .record(tool_name, &instance_id, &artifact_path, &data)
            .map_err(|e| McpError::internal_error(format!("store error: {e}"), None))?;

        let message = format!("Produced {tool_name}/{instance_id}.json");
        append_tool_event(
            "tool_result",
            &protocol_name,
            current_step.work_unit.as_deref(),
            tool_name,
            None,
            Some(&message),
        )?;
        Ok(CallToolResult::success(vec![Content::text(message)]))
    }
}

fn json_tool_result(payload: &impl serde::Serialize) -> Result<CallToolResult, McpError> {
    let json = serde_json::to_string_pretty(payload)
        .map_err(|error| McpError::internal_error(error.to_string(), None))?;
    Ok(CallToolResult::success(vec![Content::text(json)]))
}

fn extract_instance_id(data: &mut Value) -> Result<String, McpError> {
    if let Value::Object(data_map) = data {
        match data_map.remove("instance_id") {
            Some(Value::String(s)) => Ok(s),
            Some(_) => Err(McpError::invalid_params(
                "instance_id must be a string",
                None,
            )),
            None => Err(McpError::invalid_params("instance_id is required", None)),
        }
    } else {
        Err(McpError::invalid_params("instance_id is required", None))
    }
}

fn validation_message(error: libagent::ValidationError) -> String {
    match error {
        libagent::ValidationError::InvalidArtifact { violations, .. } => violations
            .iter()
            .map(|v| format!("{}: {}", v.instance_path, v.description))
            .collect::<Vec<_>>()
            .join("\n"),
        libagent::ValidationError::InvalidSchema { detail, .. } => {
            format!("schema error: {detail}")
        }
    }
}

fn output_tools_for_protocol(
    protocol: &ProtocolDeclaration,
    work_unit: Option<&str>,
    store: &ArtifactStore,
    reserve_driver_names: bool,
) -> (Vec<Tool>, HashMap<String, Value>) {
    let mut tools = Vec::new();
    let mut tool_schemas = HashMap::new();

    let output_types: Vec<&String> = protocol
        .produces
        .iter()
        .chain(protocol.required_choice_members())
        .chain(protocol.may_produce.iter().filter(|type_name| {
            if work_unit.is_none()
                && let Some(at) = store.artifact_type(type_name)
                && at.schema_requires_work_unit()
            {
                warn!(
                    operation = "tool_generation",
                    outcome = "skipped_requires_work_unit",
                    artifact_type = %type_name,
                    "skipping may_produce type because handler has no work_unit"
                );
                return false;
            }
            true
        }))
        .collect();

    for type_name in &output_types {
        if reserve_driver_names && DRIVER_TOOL_NAMES.contains(&type_name.as_str()) {
            continue;
        }
        let Some(at) = store.artifact_type(type_name) else {
            continue;
        };

        let root_type = at.schema.get("type").and_then(|t| t.as_str());
        if root_type != Some("object") {
            warn!(
                operation = "tool_generation",
                outcome = "skipped_non_object_schema",
                artifact_type = %type_name,
                schema_root_type = %root_type.unwrap_or("<missing>"),
                "skipping artifact type with unsupported schema root"
            );
            continue;
        }

        if has_composition_keywords(&at.schema) {
            warn!(
                operation = "tool_generation",
                outcome = "skipped_composed_schema",
                artifact_type = %type_name,
                "skipping artifact type with composed schema"
            );
            continue;
        }

        let stripped = strip_work_unit(&at.schema);
        let schema_obj = add_instance_id(stripped);

        tools.push(Tool::new(
            (*type_name).clone(),
            format!("Validate and write a {type_name} artifact to the workspace"),
            Arc::new(schema_obj),
        ));
        tool_schemas.insert((*type_name).clone(), at.schema.clone());
    }

    (tools, tool_schemas)
}

fn driver_tools() -> Vec<Tool> {
    let empty_schema = || {
        Arc::new(serde_json::Map::from_iter([
            ("type".to_string(), serde_json::json!("object")),
            (
                "properties".to_string(),
                Value::Object(serde_json::Map::new()),
            ),
            ("additionalProperties".to_string(), serde_json::json!(false)),
        ]))
    };
    vec![
        Tool::new(
            "readiness",
            "Reconcile and report scoped protocol readiness for this session",
            empty_schema(),
        ),
        Tool::new(
            "next-protocol-context",
            "Return the current ready protocol context and rendered prompt",
            empty_schema(),
        ),
        Tool::new(
            "advance",
            "Retire the current step after enforcing postconditions and select the next step",
            empty_schema(),
        ),
    ]
}

fn validate_session_current_step(session: &SessionState) -> Result<(), String> {
    let Some(step) = session.current_step() else {
        return Ok(());
    };
    let protocol = session
        .current_protocol()
        .map_err(|error| error.to_string())?;
    for type_name in protocol
        .produces
        .iter()
        .chain(protocol.required_choice_members())
    {
        if DRIVER_TOOL_NAMES.contains(&type_name.as_str()) {
            return Err(format!(
                "required output type '{type_name}' collides with reserved session driver verb"
            ));
        }
    }
    validate_output_types(protocol, session.store(), step.work_unit.as_deref())
}

fn session_tools_and_schemas(
    session: &SessionState,
) -> Result<(Vec<Tool>, HashMap<String, Value>), String> {
    let mut tools = driver_tools();
    let mut schemas = HashMap::new();
    if session.current_step().is_some() {
        let protocol = session
            .current_protocol()
            .map_err(|error| error.to_string())?;
        let (mut output_tools, output_schemas) = output_tools_for_protocol(
            protocol,
            session
                .current_step()
                .and_then(|step| step.work_unit.as_deref()),
            session.store(),
            true,
        );
        tools.append(&mut output_tools);
        schemas = output_schemas;
    }
    Ok((tools, schemas))
}

/// Check whether a JSON Schema uses composition keywords that prevent
/// reliable work_unit stripping and tool generation.
fn has_composition_keywords(schema: &Value) -> bool {
    schema.get("allOf").is_some()
        || schema.get("anyOf").is_some()
        || schema.get("oneOf").is_some()
        || schema.get("$ref").is_some()
}

/// Check that all `produces` types can be served as MCP tools.
///
/// Returns `Err` with a diagnostic message if any required output type has a
/// schema that cannot be converted to an MCP tool (non-object root,
/// composition keywords, or required work_unit without a scoped candidate).
pub fn validate_protocol_scope(
    protocol: &ProtocolDeclaration,
    work_unit: Option<&str>,
) -> Result<(), String> {
    match (protocol.scoped, work_unit) {
        (true, None) => Err(format!(
            "protocol '{}' requires --work-unit because it is declared scoped",
            protocol.name
        )),
        (false, Some(_)) => Err(format!(
            "protocol '{}' does not accept --work-unit because it is declared unscoped",
            protocol.name
        )),
        _ => Ok(()),
    }
}

pub fn validate_output_types(
    protocol: &ProtocolDeclaration,
    store: &ArtifactStore,
    _work_unit: Option<&str>,
) -> Result<(), String> {
    for type_name in protocol
        .produces
        .iter()
        .chain(protocol.required_choice_members())
    {
        let Some(at) = store.artifact_type(type_name) else {
            return Err(format!(
                "required output type '{type_name}' not found in manifest"
            ));
        };
        let root_type = at.schema.get("type").and_then(|t| t.as_str());
        if root_type != Some("object") {
            return Err(format!(
                "required output type '{type_name}': non-object schema root type '{}' \
                 is not supported for MCP tool generation",
                root_type.unwrap_or("<missing>")
            ));
        }
        if has_composition_keywords(&at.schema) {
            return Err(format!(
                "required output type '{type_name}': schema uses composition keywords \
                 (allOf/anyOf/oneOf/$ref); composed schemas are not supported \
                 for MCP tool generation"
            ));
        }
        if let Err(error) = validate_output_scope(protocol, at) {
            return Err(format!(
                "{error}; declare 'scoped = true' or remove 'work_unit' from the output schema's required fields"
            ));
        }
    }

    // For protocols with only optional outputs, ensure at least one may_produce
    // type can become a viable tool. If none can, the session is pointless.
    if protocol.produces.is_empty()
        && protocol.required_output_choices.is_empty()
        && !protocol.may_produce.is_empty()
    {
        let has_viable = protocol.may_produce.iter().any(|type_name| {
            let Some(at) = store.artifact_type(type_name) else {
                return false;
            };
            let root_type = at.schema.get("type").and_then(|t| t.as_str());
            if root_type != Some("object") {
                return false;
            }
            if has_composition_keywords(&at.schema) {
                return false;
            }
            if validate_output_scope(protocol, at).is_err() {
                return false;
            }
            true
        });
        if !has_viable {
            let names: Vec<&str> = protocol.may_produce.iter().map(|s| s.as_str()).collect();
            return Err(format!(
                "may_produce-only protocol has no viable output types {:?}: \
                 all schemas are unsupported for MCP tool generation",
                names
            ));
        }
    }

    Ok(())
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

/// Add `instance_id` as a required string property in the tool input schema.
///
/// The agent supplies this to name each artifact instance. It is not part of
/// the artifact's own schema — `call_tool` extracts it before validation.
fn add_instance_id(schema: Value) -> serde_json::Map<String, Value> {
    let mut map = match schema {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };

    let props = map
        .entry("properties")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if let Value::Object(props_map) = props {
        props_map.insert(
            "instance_id".to_string(),
            serde_json::json!({
                "type": "string",
                "description": "Unique identifier for this artifact instance (becomes the filename)"
            }),
        );
    }

    let required = map
        .entry("required")
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Value::Array(arr) = required
        && !arr.iter().any(|v| v.as_str() == Some("instance_id"))
    {
        arr.push(Value::String("instance_id".to_string()));
    }

    map
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
        let instructions = match &self.protocol {
            Some(protocol) => format!(
                "MCP server for protocol '{}'. Use its tools to validate and write output artifacts.",
                protocol.name
            ),
            None => "MCP server for a runa session. Use driver tools to read context and advance, and output tools to validate and write artifacts.".to_string(),
        };
        let capabilities = if self.session.is_some() {
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .build()
        } else {
            ServerCapabilities::builder().enable_tools().build()
        };
        ServerInfo {
            protocol_version: ProtocolVersion::default(),
            capabilities,
            server_info: Implementation {
                name: "runa-mcp".into(),
                version: libagent::version().into(),
            },
            instructions: Some(instructions),
        }
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        if let Some(session) = &self.session {
            let session = session.lock().unwrap();
            let (tools, _) = session_tools_and_schemas(&session)
                .map_err(|error| McpError::internal_error(error, None))?;
            return Ok(ListToolsResult {
                next_cursor: None,
                tools,
            });
        }
        Ok(ListToolsResult {
            next_cursor: None,
            tools: self.tools.clone(),
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let tool_name = request.name.to_string();
        if let Some(session) = &self.session {
            return self
                .call_session_tool(session, &tool_name, request, context)
                .await;
        }
        let protocol = self
            .protocol
            .as_ref()
            .expect("fixed protocol handler must have protocol");
        append_tool_event(
            "tool_call",
            &protocol.name,
            self.work_unit.as_deref(),
            &tool_name,
            request
                .arguments
                .as_ref()
                .map(|arguments| Value::Object(arguments.clone())),
            None,
        )?;

        // Look up the full schema for this artifact type.
        let full_schema = self
            .tool_schemas
            .get(&tool_name)
            .ok_or_else(|| McpError::invalid_params(format!("unknown tool: {tool_name}"), None))?;

        // Build the artifact data from arguments, extracting instance_id first.
        let mut data = match request.arguments {
            Some(args) => Value::Object(args),
            None => Value::Object(serde_json::Map::new()),
        };

        // Extract instance_id (tool parameter, not part of artifact schema).
        let instance_id = if let Value::Object(data_map) = &mut data {
            match data_map.remove("instance_id") {
                Some(Value::String(s)) => s,
                Some(_) => {
                    return Err(McpError::invalid_params(
                        "instance_id must be a string",
                        None,
                    ));
                }
                None => {
                    return Err(McpError::invalid_params("instance_id is required", None));
                }
            }
        } else {
            return Err(McpError::invalid_params("instance_id is required", None));
        };

        validate_instance_id(&instance_id).map_err(|e| McpError::invalid_params(e, None))?;

        let at = ArtifactType {
            name: tool_name.to_string(),
            schema: full_schema.clone(),
        };

        // Inject delegated work_unit whenever the full schema mentions it.
        if at.schema_mentions_work_unit()
            && let (Value::Object(data_map), Some(wu)) = (&mut data, &self.work_unit)
        {
            data_map.insert("work_unit".to_string(), Value::String(wu.clone()));
        }

        // Validate against the full schema (including work_unit).
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
            append_tool_event(
                "tool_result",
                &protocol.name,
                self.work_unit.as_deref(),
                &tool_name,
                None,
                Some(&msg),
            )?;
            return Ok(CallToolResult::error(vec![Content::text(msg)]));
        }

        // Write artifact to workspace.
        let type_dir = self.workspace_dir.join(&tool_name);
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
        let mut state = self
            .state
            .as_ref()
            .expect("fixed protocol handler must have state")
            .lock()
            .unwrap();
        state
            .store
            .record(&tool_name, &instance_id, &artifact_path, &data)
            .map_err(|e| McpError::internal_error(format!("store error: {e}"), None))?;

        info!(
            operation = "tool_call",
            outcome = "artifact_written",
            artifact_type = %tool_name,
            instance_id = %instance_id,
            work_unit = ?self.work_unit,
            "artifact written to workspace"
        );

        let message = format!("Produced {tool_name}/{instance_id}.json");
        append_tool_event(
            "tool_result",
            &protocol.name,
            self.work_unit.as_deref(),
            &tool_name,
            None,
            Some(&message),
        )?;

        Ok(CallToolResult::success(vec![Content::text(message)]))
    }
}

fn append_tool_event(
    kind: &'static str,
    protocol: &str,
    work_unit: Option<&str>,
    tool_name: &str,
    payload: Option<Value>,
    content: Option<&str>,
) -> Result<(), McpError> {
    libagent::transcript::append_event(libagent::transcript::TranscriptEvent {
        source: "runa-mcp",
        kind,
        protocol: Some(protocol),
        work_unit,
        tool_name: Some(tool_name),
        payload,
        content,
        ..Default::default()
    })
    .map_err(|error| {
        McpError::internal_error(format!("failed to write transcript event: {error}"), None)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use libagent::RequiredOutputChoice;
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
        let protocol = ProtocolDeclaration {
            name: "implement".into(),
            requires: vec!["constraints".into()],
            accepts: Vec::new(),
            produces: vec!["implementation".into()],
            may_produce: Vec::new(),
            required_output_choices: Vec::new(),
            scoped: false,
            trigger: TriggerCondition::OnArtifact {
                name: "constraints".into(),
            },
            instructions: None,
        };

        let handler = RunaHandler::new(
            protocol,
            Some("wu-1".into()),
            store,
            tmp.path().join("workspace"),
        );

        // Only output types become tools, not requires.
        assert_eq!(handler.tools.len(), 1);
        assert_eq!(handler.tools[0].name.as_ref(), "implementation");

        // The tool schema should not have work_unit but should have instance_id.
        let tool_props = handler.tools[0]
            .input_schema
            .get("properties")
            .and_then(|v| v.as_object())
            .expect("tool should have properties");
        assert!(!tool_props.contains_key("work_unit"));
        assert!(tool_props.contains_key("instance_id"));
        assert!(tool_props.contains_key("title"));

        // instance_id should be required.
        let required = handler.tools[0]
            .input_schema
            .get("required")
            .and_then(|v| v.as_array())
            .expect("tool should have required");
        assert!(required.iter().any(|v| v.as_str() == Some("instance_id")));
    }

    #[test]
    fn handler_derives_tools_from_required_output_choice_members() {
        let tmp = tempfile::tempdir().unwrap();
        let types = vec![
            ArtifactType {
                name: "approved".into(),
                schema: json!({
                    "type": "object",
                    "properties": { "summary": { "type": "string" } }
                }),
            },
            ArtifactType {
                name: "needs-revision".into(),
                schema: json!({
                    "type": "object",
                    "properties": { "summary": { "type": "string" } }
                }),
            },
        ];
        let store = ArtifactStore::new(types.clone(), tmp.path().join("store")).unwrap();
        let protocol = ProtocolDeclaration {
            name: "review".into(),
            requires: Vec::new(),
            accepts: Vec::new(),
            produces: Vec::new(),
            may_produce: Vec::new(),
            required_output_choices: vec![RequiredOutputChoice {
                name: "disposition".into(),
                members: vec!["approved".into(), "needs-revision".into()],
            }],
            scoped: false,
            trigger: TriggerCondition::OnChange {
                name: "approved".into(),
            },
            instructions: None,
        };

        let handler = RunaHandler::new(protocol, None, store, tmp.path().join("workspace"));
        let tool_names = handler
            .tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>();

        assert_eq!(tool_names, vec!["approved", "needs-revision"]);
    }

    #[test]
    fn handler_get_info_reports_tools_only_capabilities() {
        let tmp = tempfile::tempdir().unwrap();
        let types = vec![ArtifactType {
            name: "implementation".into(),
            schema: json!({
                "type": "object",
                "properties": { "title": { "type": "string" } }
            }),
        }];
        let store = ArtifactStore::new(types, tmp.path().join("store")).unwrap();
        let protocol = ProtocolDeclaration {
            name: "implement".into(),
            requires: Vec::new(),
            accepts: Vec::new(),
            produces: vec!["implementation".into()],
            may_produce: Vec::new(),
            required_output_choices: Vec::new(),
            scoped: false,
            trigger: TriggerCondition::OnChange {
                name: "unused".into(),
            },
            instructions: None,
        };

        let handler = RunaHandler::new(protocol, None, store, tmp.path().join("workspace"));
        let info = handler.get_info();

        assert_eq!(
            serde_json::to_value(&info.capabilities).unwrap(),
            json!({"tools": {}})
        );
        assert_eq!(
            info.instructions.as_deref(),
            Some(
                "MCP server for protocol 'implement'. Use its tools to validate and write output artifacts."
            )
        );
    }

    #[test]
    fn non_object_schema_excluded_from_tools() {
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
                    "properties": { "title": { "type": "string" } }
                }),
            },
            ArtifactType {
                name: "log_entries".into(),
                schema: json!({
                    "type": "array",
                    "items": { "type": "string" }
                }),
            },
        ];
        let store = ArtifactStore::new(types.clone(), tmp.path().join("store")).unwrap();
        let protocol = ProtocolDeclaration {
            name: "implement".into(),
            requires: vec!["constraints".into()],
            accepts: Vec::new(),
            produces: vec!["implementation".into()],
            may_produce: vec!["log_entries".into()],
            required_output_choices: Vec::new(),
            scoped: false,
            trigger: TriggerCondition::OnArtifact {
                name: "constraints".into(),
            },
            instructions: None,
        };

        let handler = RunaHandler::new(protocol, None, store, tmp.path().join("workspace"));

        // Non-object may_produce schema silently excluded; object produces included.
        assert_eq!(handler.tools.len(), 1);
        assert_eq!(handler.tools[0].name.as_ref(), "implementation");
    }

    #[test]
    fn composed_schema_excluded_from_tools() {
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
            ArtifactType {
                name: "composed".into(),
                schema: json!({
                    "type": "object",
                    "allOf": [
                        { "properties": { "title": { "type": "string" } } },
                        { "properties": { "work_unit": { "type": "string" } } }
                    ]
                }),
            },
        ];
        let store = ArtifactStore::new(types.clone(), tmp.path().join("store")).unwrap();
        let protocol = ProtocolDeclaration {
            name: "implement".into(),
            requires: vec!["constraints".into()],
            accepts: Vec::new(),
            produces: vec!["implementation".into()],
            may_produce: vec!["composed".into()],
            required_output_choices: Vec::new(),
            scoped: false,
            trigger: TriggerCondition::OnArtifact {
                name: "constraints".into(),
            },
            instructions: None,
        };

        let handler = RunaHandler::new(
            protocol,
            Some("wu-1".into()),
            store,
            tmp.path().join("workspace"),
        );

        // implementation included; composed may_produce silently excluded.
        assert_eq!(handler.tools.len(), 1);
        assert_eq!(handler.tools[0].name.as_ref(), "implementation");
    }

    #[test]
    fn all_composed_schemas_excluded() {
        let tmp = tempfile::tempdir().unwrap();
        let types = vec![
            ArtifactType {
                name: "composed_a".into(),
                schema: json!({
                    "type": "object",
                    "allOf": [
                        { "properties": { "title": { "type": "string" } } }
                    ]
                }),
            },
            ArtifactType {
                name: "composed_b".into(),
                schema: json!({
                    "type": "object",
                    "oneOf": [
                        { "properties": { "x": { "type": "integer" } } }
                    ]
                }),
            },
        ];
        let store = ArtifactStore::new(types.clone(), tmp.path().join("store")).unwrap();
        let protocol = ProtocolDeclaration {
            name: "compose-all".into(),
            requires: Vec::new(),
            accepts: Vec::new(),
            produces: Vec::new(),
            may_produce: vec!["composed_a".into(), "composed_b".into()],
            required_output_choices: Vec::new(),
            scoped: false,
            trigger: TriggerCondition::OnChange {
                name: "unused".into(),
            },
            instructions: None,
        };

        let handler = RunaHandler::new(protocol, None, store, tmp.path().join("workspace"));

        // All output types use composition → all excluded.
        assert!(handler.tools.is_empty());
    }

    #[test]
    fn validate_output_types_rejects_non_object_produces() {
        let tmp = tempfile::tempdir().unwrap();
        let types = vec![ArtifactType {
            name: "log_entries".into(),
            schema: json!({
                "type": "array",
                "items": { "type": "string" }
            }),
        }];
        let store = ArtifactStore::new(types.clone(), tmp.path().join("store")).unwrap();
        let protocol = ProtocolDeclaration {
            name: "log".into(),
            requires: Vec::new(),
            accepts: Vec::new(),
            produces: vec!["log_entries".into()],
            may_produce: Vec::new(),
            required_output_choices: Vec::new(),
            scoped: false,
            trigger: TriggerCondition::OnChange {
                name: "unused".into(),
            },
            instructions: None,
        };

        let result = validate_output_types(&protocol, &store, Some("wu"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("non-object schema root type"));
    }

    #[test]
    fn validate_output_types_rejects_composed_produces() {
        let tmp = tempfile::tempdir().unwrap();
        let types = vec![ArtifactType {
            name: "composed".into(),
            schema: json!({
                "type": "object",
                "allOf": [
                    { "properties": { "title": { "type": "string" } } }
                ]
            }),
        }];
        let store = ArtifactStore::new(types.clone(), tmp.path().join("store")).unwrap();
        let protocol = ProtocolDeclaration {
            name: "compose".into(),
            requires: Vec::new(),
            accepts: Vec::new(),
            produces: vec!["composed".into()],
            may_produce: Vec::new(),
            required_output_choices: Vec::new(),
            scoped: false,
            trigger: TriggerCondition::OnChange {
                name: "unused".into(),
            },
            instructions: None,
        };

        let result = validate_output_types(&protocol, &store, Some("wu"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("composition keywords"));
    }

    #[test]
    fn validate_output_types_accepts_valid_produces() {
        let tmp = tempfile::tempdir().unwrap();
        let types = vec![
            ArtifactType {
                name: "output_a".into(),
                schema: json!({
                    "type": "object",
                    "properties": { "title": { "type": "string" } }
                }),
            },
            ArtifactType {
                name: "output_b".into(),
                schema: json!({ "type": "object" }),
            },
        ];
        let store = ArtifactStore::new(types.clone(), tmp.path().join("store")).unwrap();
        let protocol = ProtocolDeclaration {
            name: "produce".into(),
            requires: Vec::new(),
            accepts: Vec::new(),
            produces: vec!["output_a".into(), "output_b".into()],
            may_produce: Vec::new(),
            required_output_choices: Vec::new(),
            scoped: false,
            trigger: TriggerCondition::OnChange {
                name: "unused".into(),
            },
            instructions: None,
        };

        assert!(validate_output_types(&protocol, &store, Some("wu")).is_ok());
    }

    #[test]
    fn validate_output_types_rejects_required_work_unit_when_unscoped() {
        let tmp = tempfile::tempdir().unwrap();
        let types = vec![ArtifactType {
            name: "implementation".into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "work_unit": { "type": "string" }
                },
                "required": ["title", "work_unit"]
            }),
        }];
        let store = ArtifactStore::new(types.clone(), tmp.path().join("store")).unwrap();
        let protocol = ProtocolDeclaration {
            name: "implement".into(),
            requires: Vec::new(),
            accepts: Vec::new(),
            produces: vec!["implementation".into()],
            may_produce: Vec::new(),
            required_output_choices: Vec::new(),
            scoped: false,
            trigger: TriggerCondition::OnChange {
                name: "unused".into(),
            },
            instructions: None,
        };

        let result = validate_output_types(&protocol, &store, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("requires 'work_unit'"));
    }

    #[test]
    fn validate_output_types_accepts_required_work_unit_when_scoped() {
        let tmp = tempfile::tempdir().unwrap();
        let types = vec![ArtifactType {
            name: "implementation".into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "work_unit": { "type": "string" }
                },
                "required": ["title", "work_unit"]
            }),
        }];
        let store = ArtifactStore::new(types.clone(), tmp.path().join("store")).unwrap();
        let protocol = ProtocolDeclaration {
            name: "implement".into(),
            requires: Vec::new(),
            accepts: Vec::new(),
            produces: vec!["implementation".into()],
            may_produce: Vec::new(),
            required_output_choices: Vec::new(),
            scoped: true,
            trigger: TriggerCondition::OnChange {
                name: "unused".into(),
            },
            instructions: None,
        };

        assert!(validate_output_types(&protocol, &store, Some("wu")).is_ok());
    }

    #[test]
    fn validate_output_types_accepts_scoped_project_level_output() {
        let tmp = tempfile::tempdir().unwrap();
        let types = vec![ArtifactType {
            name: "implementation".into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" }
                },
                "required": ["title"]
            }),
        }];
        let store = ArtifactStore::new(types.clone(), tmp.path().join("store")).unwrap();
        let protocol = ProtocolDeclaration {
            name: "implement".into(),
            requires: Vec::new(),
            accepts: Vec::new(),
            produces: vec!["implementation".into()],
            may_produce: Vec::new(),
            required_output_choices: Vec::new(),
            scoped: true,
            trigger: TriggerCondition::OnChange {
                name: "unused".into(),
            },
            instructions: None,
        };

        assert!(validate_output_types(&protocol, &store, Some("wu")).is_ok());
    }

    #[test]
    fn handler_skips_may_produce_requiring_work_unit_when_unscoped() {
        let tmp = tempfile::tempdir().unwrap();
        let types = vec![
            ArtifactType {
                name: "output".into(),
                schema: json!({
                    "type": "object",
                    "properties": { "title": { "type": "string" } }
                }),
            },
            ArtifactType {
                name: "scoped_output".into(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string" },
                        "work_unit": { "type": "string" }
                    },
                    "required": ["title", "work_unit"]
                }),
            },
        ];
        let store = ArtifactStore::new(types.clone(), tmp.path().join("store")).unwrap();
        let protocol = ProtocolDeclaration {
            name: "produce".into(),
            requires: Vec::new(),
            accepts: Vec::new(),
            produces: vec!["output".into()],
            may_produce: vec!["scoped_output".into()],
            required_output_choices: Vec::new(),
            scoped: false,
            trigger: TriggerCondition::OnChange {
                name: "unused".into(),
            },
            instructions: None,
        };

        let handler = RunaHandler::new(
            protocol,
            None, // unscoped
            store,
            tmp.path().join("workspace"),
        );

        // Only "output" should be a tool; "scoped_output" filtered out.
        assert_eq!(handler.tools.len(), 1);
        assert_eq!(handler.tools[0].name.as_ref(), "output");
    }

    #[test]
    fn validate_protocol_scope_requires_work_unit_for_scoped_protocols() {
        let protocol = ProtocolDeclaration {
            name: "implement".into(),
            requires: Vec::new(),
            accepts: Vec::new(),
            produces: Vec::new(),
            may_produce: Vec::new(),
            required_output_choices: Vec::new(),
            scoped: true,
            trigger: TriggerCondition::OnChange {
                name: "unused".into(),
            },
            instructions: None,
        };

        let error = validate_protocol_scope(&protocol, None).unwrap_err();
        assert!(error.contains("requires --work-unit"));
        assert!(validate_protocol_scope(&protocol, Some("wu-a")).is_ok());
    }

    #[test]
    fn validate_protocol_scope_rejects_work_unit_for_unscoped_protocols() {
        let protocol = ProtocolDeclaration {
            name: "ground".into(),
            requires: Vec::new(),
            accepts: Vec::new(),
            produces: Vec::new(),
            may_produce: Vec::new(),
            required_output_choices: Vec::new(),
            scoped: false,
            trigger: TriggerCondition::OnChange {
                name: "unused".into(),
            },
            instructions: None,
        };

        let error = validate_protocol_scope(&protocol, Some("wu-a")).unwrap_err();
        assert!(error.contains("does not accept --work-unit"));
        assert!(validate_protocol_scope(&protocol, None).is_ok());
    }
}
