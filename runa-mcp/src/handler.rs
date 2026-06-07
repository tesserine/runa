use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rmcp::Error as McpError;
use rmcp::ServerHandler;
use rmcp::model::*;
use rmcp::service::{RequestContext, RoleServer};
use serde::Serialize;
use serde_json::Value;
use tracing::{info, warn};

use libagent::context::{ContextInjectionView, render_context_prompt};
use libagent::validation::validate_artifact;
use libagent::{
    ArtifactStore, ArtifactType, LoadedProject, ProtocolDeclaration, SessionPlanEntry,
    validate_output_scope,
};

pub struct RunaHandler {
    protocol: ProtocolDeclaration,
    work_unit: Option<String>,
    state: Mutex<HandlerState>,
    workspace_dir: PathBuf,
    tools: Vec<Tool>,
    /// Maps artifact type name → full JSON Schema (with work_unit intact).
    tool_schemas: HashMap<String, Value>,
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
        let (tools, tool_schemas) = build_output_tools(&protocol, work_unit.as_deref(), &store);

        Self {
            protocol,
            work_unit,
            state: Mutex::new(HandlerState { store }),
            workspace_dir,
            tools,
            tool_schemas,
        }
    }
}

pub struct SessionHandler {
    work_unit: String,
    working_dir: PathBuf,
    state: Mutex<SessionHandlerState>,
    workspace_dir: PathBuf,
}

struct SessionHandlerState {
    loaded: LoadedProject,
    pending: Option<SessionPlanEntry>,
}

#[derive(Serialize)]
struct ReadinessPayload {
    version: u32,
    methodology: String,
    scan_warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cycle: Option<Vec<String>>,
    protocols: Vec<libagent::ProtocolJson>,
}

#[derive(Serialize)]
struct ContextPayload {
    protocol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    work_unit: Option<String>,
    trigger: String,
    context: ContextInjectionView,
    rendered_prompt: String,
}

impl SessionHandler {
    pub fn new(
        loaded: LoadedProject,
        working_dir: PathBuf,
        work_unit: String,
        workspace_dir: PathBuf,
    ) -> Result<Self, String> {
        let session_state = evaluate_loaded_session(&loaded, &working_dir, &work_unit);
        let pending = session_state.planned_entries.into_iter().next();
        validate_session_pending(&loaded, pending.as_ref(), &work_unit)?;

        Ok(Self {
            work_unit,
            working_dir,
            state: Mutex::new(SessionHandlerState { loaded, pending }),
            workspace_dir,
        })
    }

    fn refresh_state(&self) -> Result<libagent::SessionState, McpError> {
        let mut state = self.state.lock().unwrap();
        let scan_result =
            libagent::scan(&self.workspace_dir, &mut state.loaded.store).map_err(|error| {
                McpError::internal_error(format!("failed to scan workspace: {error}"), None)
            })?;
        let session_state = libagent::evaluate_session_state(
            &state.loaded,
            &self.working_dir,
            &scan_result,
            libagent::EvaluationScope::Scoped(&self.work_unit),
        );
        state.pending = session_state.planned_entries.first().cloned();
        validate_session_pending(&state.loaded, state.pending.as_ref(), &self.work_unit)
            .map_err(|error| McpError::internal_error(error, None))?;
        Ok(session_state)
    }

    fn readiness_result(&self) -> Result<CallToolResult, McpError> {
        let session_state = self.refresh_state()?;
        let methodology = self.state.lock().unwrap().loaded.manifest.name.clone();
        readiness_call_result(methodology, session_state)
    }

    fn next_context_result(&self) -> Result<CallToolResult, McpError> {
        let session_state = self.refresh_state()?;
        let Some(entry) = session_state.planned_entries.into_iter().next() else {
            return Ok(CallToolResult::error(vec![Content::text(
                "No READY protocols.",
            )]));
        };
        let payload = ContextPayload {
            protocol: entry.protocol.clone(),
            work_unit: entry.work_unit.clone(),
            trigger: entry.trigger,
            rendered_prompt: render_context_prompt(&entry.context),
            context: ContextInjectionView::from(&entry.context),
        };
        let json = serde_json::to_string_pretty(&payload).map_err(|error| {
            McpError::internal_error(format!("failed to serialize context: {error}"), None)
        })?;
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    fn advance_result(&self) -> Result<CallToolResult, McpError> {
        let mut state = self.state.lock().unwrap();
        let pending = match state.pending.clone() {
            Some(pending) => pending,
            None => {
                let scan_result = libagent::scan(&self.workspace_dir, &mut state.loaded.store)
                    .map_err(|error| {
                        McpError::internal_error(format!("failed to scan workspace: {error}"), None)
                    })?;
                let session_state = libagent::evaluate_session_state(
                    &state.loaded,
                    &self.working_dir,
                    &scan_result,
                    libagent::EvaluationScope::Scoped(&self.work_unit),
                );
                let pending = session_state.planned_entries.first().cloned();
                state.pending = pending.clone();
                match pending {
                    Some(pending) => pending,
                    None => {
                        return Ok(CallToolResult::error(vec![Content::text(
                            "No READY protocols.",
                        )]));
                    }
                }
            }
        };

        let scan_result =
            libagent::scan(&self.workspace_dir, &mut state.loaded.store).map_err(|error| {
                McpError::internal_error(format!("failed to scan workspace: {error}"), None)
            })?;

        let protocol = state
            .loaded
            .manifest
            .protocols
            .iter()
            .find(|protocol| protocol.name == pending.protocol)
            .cloned()
            .ok_or_else(|| {
                McpError::internal_error(
                    format!("planned protocol '{}' is missing", pending.protocol),
                    None,
                )
            })?;

        if let Err(error) = libagent::enforce_postconditions(
            &protocol,
            &state.loaded.store,
            pending.work_unit.as_deref(),
        ) {
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "postconditions failed for protocol '{}': {error}",
                pending.protocol
            ))]));
        }

        state
            .loaded
            .store
            .record_execution(
                &pending.protocol,
                pending.work_unit.as_deref(),
                pending.execution_record,
            )
            .map_err(|error| {
                McpError::internal_error(
                    format!("failed to record execution metadata: {error}"),
                    None,
                )
            })?;

        let session_state = libagent::evaluate_session_state(
            &state.loaded,
            &self.working_dir,
            &scan_result,
            libagent::EvaluationScope::Scoped(&self.work_unit),
        );
        state.pending = session_state.planned_entries.first().cloned();
        validate_session_pending(&state.loaded, state.pending.as_ref(), &self.work_unit)
            .map_err(|error| McpError::internal_error(error, None))?;
        let methodology = state.loaded.manifest.name.clone();
        drop(state);
        readiness_call_result(methodology, session_state)
    }

    fn current_tools_and_schemas(&self) -> Result<(Vec<Tool>, HashMap<String, Value>), McpError> {
        let state = self.state.lock().unwrap();
        session_tools_and_schemas(&state, &self.work_unit)
    }
}

fn readiness_call_result(
    methodology: String,
    session_state: libagent::SessionState,
) -> Result<CallToolResult, McpError> {
    let payload = ReadinessPayload {
        version: 1,
        methodology,
        scan_warnings: session_state.scan_findings.warnings.clone(),
        cycle: session_state
            .evaluated
            .cycle
            .as_ref()
            .map(|cycle| cycle.path.clone()),
        protocols: session_state.evaluated.json_protocols(),
    };
    let json = serde_json::to_string_pretty(&payload).map_err(|error| {
        McpError::internal_error(format!("failed to serialize readiness: {error}"), None)
    })?;
    Ok(CallToolResult::success(vec![Content::text(json)]))
}

fn evaluate_loaded_session(
    loaded: &LoadedProject,
    working_dir: &Path,
    work_unit: &str,
) -> libagent::SessionState {
    let scan_result = libagent::ScanResult::default();
    libagent::evaluate_session_state(
        loaded,
        working_dir,
        &scan_result,
        libagent::EvaluationScope::Scoped(work_unit),
    )
}

fn validate_session_pending(
    loaded: &LoadedProject,
    pending: Option<&SessionPlanEntry>,
    work_unit: &str,
) -> Result<(), String> {
    let Some(entry) = pending else {
        return Ok(());
    };
    let protocol = loaded
        .manifest
        .protocols
        .iter()
        .find(|protocol| protocol.name == entry.protocol)
        .ok_or_else(|| format!("planned protocol '{}' is missing", entry.protocol))?;
    validate_output_types(protocol, &loaded.store, Some(work_unit)).map_err(|error| {
        format!(
            "protocol '{}' cannot be served via MCP tools: {error}",
            protocol.name
        )
    })
}

fn session_tools_and_schemas(
    state: &SessionHandlerState,
    work_unit: &str,
) -> Result<(Vec<Tool>, HashMap<String, Value>), McpError> {
    let mut tools = driver_tools();
    let mut tool_schemas = HashMap::new();

    if let Some(entry) = &state.pending {
        let protocol = state
            .loaded
            .manifest
            .protocols
            .iter()
            .find(|protocol| protocol.name == entry.protocol)
            .ok_or_else(|| {
                McpError::internal_error(
                    format!("planned protocol '{}' is missing", entry.protocol),
                    None,
                )
            })?;
        validate_output_types(protocol, &state.loaded.store, Some(work_unit)).map_err(|error| {
            McpError::internal_error(
                format!(
                    "protocol '{}' cannot be served via MCP tools: {error}",
                    protocol.name
                ),
                None,
            )
        })?;
        let (output_tools, output_schemas) =
            build_output_tools(protocol, Some(work_unit), &state.loaded.store);
        tools.extend(output_tools);
        tool_schemas.extend(output_schemas);
    }

    Ok((tools, tool_schemas))
}

fn driver_tools() -> Vec<Tool> {
    vec![
        driver_tool("readiness", "Report session readiness for the active scope"),
        driver_tool(
            "next-protocol-context",
            "Return the next ready protocol context for the active scope",
        ),
        driver_tool(
            "advance",
            "Reconcile produced outputs, record execution, and refresh readiness",
        ),
    ]
}

fn driver_tool(name: &'static str, description: &'static str) -> Tool {
    Tool::new(
        name,
        description,
        Arc::new(serde_json::Map::from_iter([
            ("type".to_string(), Value::String("object".to_string())),
            (
                "properties".to_string(),
                Value::Object(serde_json::Map::new()),
            ),
        ])),
    )
}

fn build_output_tools(
    protocol: &ProtocolDeclaration,
    work_unit: Option<&str>,
    store: &ArtifactStore,
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

fn call_output_tool(
    protocol_name: &str,
    work_unit: Option<&str>,
    request: CallToolRequestParam,
    tool_schemas: &HashMap<String, Value>,
    store: &mut ArtifactStore,
    workspace_dir: &Path,
) -> Result<CallToolResult, McpError> {
    let tool_name = request.name.as_ref();
    append_tool_event(
        "tool_call",
        protocol_name,
        work_unit,
        tool_name,
        request
            .arguments
            .as_ref()
            .map(|arguments| Value::Object(arguments.clone())),
        None,
    )?;

    let full_schema = tool_schemas
        .get(tool_name)
        .ok_or_else(|| McpError::invalid_params(format!("unknown tool: {tool_name}"), None))?;

    let mut data = match request.arguments {
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

impl ServerHandler for RunaHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::default(),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "runa-mcp".into(),
                version: libagent::version().into(),
            },
            instructions: Some(format!(
                "MCP server for protocol '{}'. Use its tools to validate and write output artifacts.",
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
        let mut state = self.state.lock().unwrap();
        call_output_tool(
            &self.protocol.name,
            self.work_unit.as_deref(),
            request,
            &self.tool_schemas,
            &mut state.store,
            &self.workspace_dir,
        )
    }
}

impl ServerHandler for SessionHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::default(),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "runa-mcp".into(),
                version: libagent::version().into(),
            },
            instructions: Some(format!(
                "MCP session surface for work unit '{}'. Use readiness, next-protocol-context, advance, and protocol output tools.",
                self.work_unit
            )),
        }
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let (tools, _) = self.current_tools_and_schemas()?;
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
        let tool_name = request.name.to_string();
        match tool_name.as_str() {
            "readiness" => self.readiness_result(),
            "next-protocol-context" => self.next_context_result(),
            "advance" => self.advance_result(),
            _ => {
                let mut state = self.state.lock().unwrap();
                let protocol_name = state
                    .pending
                    .as_ref()
                    .map(|entry| entry.protocol.clone())
                    .unwrap_or_else(|| "<session>".to_string());
                let (_, tool_schemas) = session_tools_and_schemas(&state, &self.work_unit)?;
                call_output_tool(
                    &protocol_name,
                    Some(&self.work_unit),
                    request,
                    &tool_schemas,
                    &mut state.loaded.store,
                    &self.workspace_dir,
                )
            }
        }
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
