use std::collections::HashMap;
use std::path::PathBuf;
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
    ArtifactStore, ArtifactType, PlannedEntry, ProtocolDeclaration, Session, validate_output_scope,
};

const DRIVER_TOOLS: [&str; 3] = ["readiness", "next-protocol-context", "advance"];

pub struct RunaHandler {
    mode: Mutex<HandlerMode>,
    workspace_dir: PathBuf,
    #[cfg(test)]
    tools: Vec<Tool>,
}

enum HandlerMode {
    Protocol(Box<ProtocolHandlerState>),
    Session(Box<SessionHandlerState>),
}

struct ProtocolHandlerState {
    protocol: ProtocolDeclaration,
    work_unit: Option<String>,
    store: ArtifactStore,
    tools: Vec<Tool>,
    tool_schemas: HashMap<String, Value>,
}

struct SessionHandlerState {
    session: Session,
    tools: Vec<Tool>,
    tool_schemas: HashMap<String, Value>,
}

impl RunaHandler {
    pub fn new(
        protocol: ProtocolDeclaration,
        work_unit: Option<String>,
        store: ArtifactStore,
        workspace_dir: PathBuf,
    ) -> Self {
        let (tools, tool_schemas) =
            output_tools_for_protocol(&protocol, &store, work_unit.as_deref());

        Self {
            mode: Mutex::new(HandlerMode::Protocol(Box::new(ProtocolHandlerState {
                protocol,
                work_unit,
                store,
                tools: tools.clone(),
                tool_schemas,
            }))),
            workspace_dir,
            #[cfg(test)]
            tools,
        }
    }

    pub fn new_session(session: Session, workspace_dir: PathBuf) -> Result<Self, String> {
        let (tools, tool_schemas) = session_tools(&session)?;
        Ok(Self {
            mode: Mutex::new(HandlerMode::Session(Box::new(SessionHandlerState {
                session,
                tools: tools.clone(),
                tool_schemas,
            }))),
            workspace_dir,
            #[cfg(test)]
            tools,
        })
    }
}

fn output_tools_for_protocol(
    protocol: &ProtocolDeclaration,
    store: &ArtifactStore,
    work_unit: Option<&str>,
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
    DRIVER_TOOLS
        .iter()
        .map(|name| {
            Tool::new(
                *name,
                match *name {
                    "readiness" => "Report session readiness for the current scope",
                    "next-protocol-context" => {
                        "Return the current step context and rendered prompt"
                    }
                    "advance" => {
                        "Enforce the current step, record execution metadata, and select the next step"
                    }
                    _ => unreachable!("fixed driver tool names"),
                },
                Arc::new(empty_object_schema()),
            )
        })
        .collect()
}

fn empty_object_schema() -> serde_json::Map<String, Value> {
    serde_json::json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false
    })
    .as_object()
    .expect("driver schema must be an object")
    .clone()
}

fn session_tools(session: &Session) -> Result<(Vec<Tool>, HashMap<String, Value>), String> {
    let mut tools = driver_tools();
    let mut schemas = HashMap::new();
    if let Some(step) = session.current_step() {
        let (output_tools, output_schemas) = tools_for_step(step, session)?;
        tools.extend(output_tools);
        schemas = output_schemas;
    }
    Ok((tools, schemas))
}

fn tools_for_step(
    step: &PlannedEntry,
    session: &Session,
) -> Result<(Vec<Tool>, HashMap<String, Value>), String> {
    let current_protocol = session
        .protocol(&step.protocol)
        .ok_or_else(|| format!("protocol '{}' not found in manifest", step.protocol))?;
    validate_session_step_outputs(current_protocol, session.store(), step.work_unit.as_deref())?;
    Ok(output_tools_for_protocol(
        current_protocol,
        session.store(),
        step.work_unit.as_deref(),
    ))
}

pub fn validate_session_current_step(
    step: &PlannedEntry,
    loaded: &libagent::LoadedProject,
) -> Result<(), String> {
    let protocol = loaded
        .manifest
        .protocols
        .iter()
        .find(|protocol| protocol.name == step.protocol)
        .ok_or_else(|| format!("protocol '{}' not found in manifest", step.protocol))?;
    validate_session_step_outputs(protocol, &loaded.store, step.work_unit.as_deref())
}

fn validate_session_step_outputs(
    protocol: &ProtocolDeclaration,
    store: &ArtifactStore,
    work_unit: Option<&str>,
) -> Result<(), String> {
    for type_name in protocol
        .produces
        .iter()
        .chain(protocol.required_choice_members())
    {
        if DRIVER_TOOLS.contains(&type_name.as_str()) {
            return Err(format!(
                "required output type '{type_name}' collides with reserved session driver verb"
            ));
        }
    }
    for type_name in &protocol.may_produce {
        if DRIVER_TOOLS.contains(&type_name.as_str())
            && may_produce_output_is_advertised(type_name, store, work_unit)
        {
            return Err(format!(
                "optional output type '{type_name}' collides with reserved session driver verb"
            ));
        }
    }
    validate_output_types(protocol, store, work_unit)
}

fn may_produce_output_is_advertised(
    type_name: &str,
    store: &ArtifactStore,
    work_unit: Option<&str>,
) -> bool {
    let Some(at) = store.artifact_type(type_name) else {
        return false;
    };
    if work_unit.is_none() && at.schema_requires_work_unit() {
        return false;
    }
    at.schema.get("type").and_then(|t| t.as_str()) == Some("object")
        && !has_composition_keywords(&at.schema)
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
        let instructions = match &*self.mode.lock().unwrap() {
            HandlerMode::Protocol(state) => format!(
                "MCP server for protocol '{}'. Use its tools to validate and write output artifacts.",
                state.protocol.name
            ),
            HandlerMode::Session(_) => {
                "MCP session surface. Use driver tools to read readiness, retrieve context, and advance; use output tools to produce artifacts for the current step.".to_string()
            }
        };
        ServerInfo {
            protocol_version: ProtocolVersion::default(),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
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
        let tools = match &*self.mode.lock().unwrap() {
            HandlerMode::Protocol(state) => state.tools.clone(),
            HandlerMode::Session(state) => state.tools.clone(),
        };
        Ok(ListToolsResult {
            next_cursor: None,
            tools,
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let tool_name = request.name.as_ref();
        let mut mode = self.mode.lock().unwrap();
        match &mut *mode {
            HandlerMode::Protocol(state) => call_output_tool(
                &state.protocol.name,
                state.work_unit.as_deref(),
                tool_name,
                request.arguments,
                &state.tool_schemas,
                &mut state.store,
                &self.workspace_dir,
            ),
            HandlerMode::Session(state) if DRIVER_TOOLS.contains(&tool_name) => {
                call_driver_tool(tool_name, request.arguments, state)
            }
            HandlerMode::Session(state) => {
                let (protocol, work_unit) = state
                    .session
                    .current_step()
                    .map(|step| (step.protocol.clone(), step.work_unit.clone()))
                    .unwrap_or_else(|| ("<session>".to_string(), None));
                call_output_tool(
                    &protocol,
                    work_unit.as_deref(),
                    tool_name,
                    request.arguments,
                    &state.tool_schemas,
                    state.session.store_mut(),
                    &self.workspace_dir,
                )
            }
        }
    }
}

fn call_driver_tool(
    tool_name: &str,
    arguments: Option<serde_json::Map<String, Value>>,
    state: &mut SessionHandlerState,
) -> Result<CallToolResult, McpError> {
    let (protocol, work_unit) = current_protocol_and_work_unit(&state.session);
    append_tool_event(
        "tool_call",
        &protocol,
        work_unit.as_deref(),
        tool_name,
        arguments.clone().map(Value::Object),
        None,
    )?;

    if arguments.as_ref().is_some_and(|args| !args.is_empty()) {
        return Err(McpError::invalid_params(
            format!("{tool_name} does not accept arguments"),
            None,
        ));
    }

    let payload = match tool_name {
        "readiness" => {
            let current_step = state
                .session
                .current_step()
                .map(libagent::StepSummary::from);
            let readiness = state.session.readiness().map_err(session_mcp_error)?;
            serde_json::json!({
                "current_step": current_step,
                "readiness": readiness,
            })
        }
        "next-protocol-context" => {
            let current_step = state
                .session
                .current_step()
                .map(libagent::StepSummary::from);
            let context = state.session.next_context().map_err(session_mcp_error)?;
            let prompt = context
                .as_ref()
                .map(libagent::context::render_context_prompt);
            let context_view = context.as_ref().map(ContextInjectionView::from);
            let readiness = state.session.readiness().map_err(session_mcp_error)?;
            serde_json::json!({
                "current_step": current_step,
                "context": context_view,
                "prompt": prompt,
                "readiness": readiness,
            })
        }
        "advance" => {
            let report = state
                .session
                .advance(validate_session_current_step)
                .map_err(session_mcp_error)?;
            let (tools, schemas) = session_tools(&state.session).map_err(|error| {
                McpError::internal_error(format!("failed to refresh session tools: {error}"), None)
            })?;
            state.tools = tools;
            state.tool_schemas = schemas;
            serde_json::to_value(report).map_err(|error| {
                McpError::internal_error(
                    format!("failed to serialize advance report: {error}"),
                    None,
                )
            })?
        }
        _ => {
            return Err(McpError::invalid_params(
                format!("unknown tool: {tool_name}"),
                None,
            ));
        }
    };

    let content = Content::json(payload)?;
    append_tool_event(
        "tool_result",
        &protocol,
        work_unit.as_deref(),
        tool_name,
        None,
        Some("ok"),
    )?;
    Ok(CallToolResult::success(vec![content]))
}

fn current_protocol_and_work_unit(session: &Session) -> (String, Option<String>) {
    session
        .current_step()
        .map(|step| (step.protocol.clone(), step.work_unit.clone()))
        .unwrap_or_else(|| ("<session>".to_string(), None))
}

fn session_mcp_error(error: libagent::SessionError) -> McpError {
    McpError::internal_error(error.to_string(), None)
}

fn call_output_tool(
    protocol_name: &str,
    work_unit: Option<&str>,
    tool_name: &str,
    arguments: Option<serde_json::Map<String, Value>>,
    tool_schemas: &HashMap<String, Value>,
    store: &mut ArtifactStore,
    workspace_dir: &std::path::Path,
) -> Result<CallToolResult, McpError> {
    append_tool_event(
        "tool_call",
        protocol_name,
        work_unit,
        tool_name,
        arguments
            .as_ref()
            .map(|arguments| Value::Object(arguments.clone())),
        None,
    )?;

    let full_schema = tool_schemas
        .get(tool_name)
        .ok_or_else(|| McpError::invalid_params(format!("unknown tool: {tool_name}"), None))?;

    let mut data = match arguments {
        Some(args) => Value::Object(args),
        None => Value::Object(serde_json::Map::new()),
    };

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

    if at.schema_mentions_work_unit()
        && let (Value::Object(data_map), Some(wu)) = (&mut data, work_unit)
    {
        data_map.insert("work_unit".to_string(), Value::String(wu.to_string()));
    }

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
            protocol_name,
            work_unit,
            tool_name,
            None,
            Some(&msg),
        )?;
        return Ok(CallToolResult::error(vec![Content::text(msg)]));
    }

    let type_dir = workspace_dir.join(tool_name);
    std::fs::create_dir_all(&type_dir)
        .map_err(|e| McpError::internal_error(format!("failed to create directory: {e}"), None))?;
    let artifact_path = type_dir.join(format!("{instance_id}.json"));
    let json = serde_json::to_string_pretty(&data)
        .map_err(|e| McpError::internal_error(format!("serialization error: {e}"), None))?;
    std::fs::write(&artifact_path, &json)
        .map_err(|e| McpError::internal_error(format!("failed to write artifact: {e}"), None))?;

    store
        .record(tool_name, &instance_id, &artifact_path, &data)
        .map_err(|e| McpError::internal_error(format!("store error: {e}"), None))?;

    info!(
        operation = "tool_call",
        outcome = "artifact_written",
        artifact_type = %tool_name,
        instance_id = %instance_id,
        work_unit = ?work_unit,
        "artifact written to workspace"
    );

    let message = format!("Produced {tool_name}/{instance_id}.json");
    append_tool_event(
        "tool_result",
        protocol_name,
        work_unit,
        tool_name,
        None,
        Some(&message),
    )?;

    Ok(CallToolResult::success(vec![Content::text(message)]))
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
