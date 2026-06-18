use libagent::{ForgeConfig, ResolvedForgeIdentity};
use runa_forge_contract::{ComposedTool, ForgeConnector, ForgeError, Operation, compose_tool_sets};
use runa_forge_github::{GithubConfig, GithubConnector, GithubHttpTransport};
use runa_forge_sourcehut::{SourcehutConfig, SourcehutConnector, SourcehutHttpTransport};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::fmt;

pub struct ForgeRuntime {
    pub tools: BTreeMap<String, ComposedTool>,
    connector: Box<dyn ForgeConnector>,
}

impl ForgeRuntime {
    pub fn call_tool(&self, exposed_name: &str, input: Value) -> Result<Value, RuntimeError> {
        let tool = self
            .tools
            .get(exposed_name)
            .ok_or_else(|| RuntimeError::UnknownTool(exposed_name.to_string()))?;
        validate_tool_input(tool, &input)?;
        self.connector
            .call(tool.operation, input)
            .map_err(RuntimeError::Forge)
    }
}

fn validate_tool_input(tool: &ComposedTool, input: &Value) -> Result<(), RuntimeError> {
    let validator = jsonschema::validator_for(&tool.input_schema).map_err(|error| {
        RuntimeError::InvalidInput(format!(
            "{} input schema is invalid: {error}",
            tool.exposed_name
        ))
    })?;
    let violations = validator
        .iter_errors(input)
        .map(|error| format!("{}: {}", error.instance_path(), error))
        .collect::<Vec<_>>();
    if violations.is_empty() {
        Ok(())
    } else {
        Err(RuntimeError::InvalidInput(format!(
            "{} input does not match advertised schema:\n{}",
            tool.exposed_name,
            violations.join("\n")
        )))
    }
}

#[derive(Debug)]
pub enum RuntimeError {
    UnknownTool(String),
    Composition(String),
    InvalidInput(String),
    Forge(ForgeError),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuntimeError::UnknownTool(tool) => write!(f, "unknown forge tool '{tool}'"),
            RuntimeError::Composition(message) => write!(f, "{message}"),
            RuntimeError::InvalidInput(message) => write!(f, "invalid input: {message}"),
            RuntimeError::Forge(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for RuntimeError {}

pub fn runtime_from_config(config: &ForgeConfig) -> Result<Option<ForgeRuntime>, RuntimeError> {
    let identity = libagent::resolve_forge_identity(config);
    runtime_from_config_with_identity(config, &identity)
}

pub fn runtime_from_config_with_identity(
    config: &ForgeConfig,
    identity: &ResolvedForgeIdentity,
) -> Result<Option<ForgeRuntime>, RuntimeError> {
    if !has_connector_identity_config(config) {
        return Ok(None);
    }
    let forge_type = identity.forge_type.as_str();
    let aliases: HashMap<String, String> = config
        .tool_aliases
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();

    match forge_type {
        "github" => {
            let Some(owner) = identity.owner.as_ref().filter(|value| !value.is_empty()) else {
                return Ok(None);
            };
            let Some(repo) = identity.name.as_ref().filter(|value| !value.is_empty()) else {
                return Ok(None);
            };
            let connector = GithubConnector::new(
                GithubConfig {
                    owner: owner.clone(),
                    repo: repo.clone(),
                    api_base: config
                        .api_base
                        .clone()
                        .unwrap_or_else(|| "https://api.github.com".to_string()),
                    assignee: config.assignee.clone(),
                    credential_env: config.credential_env.clone(),
                    credential_command: non_empty_command(config),
                },
                GithubHttpTransport,
            );
            runtime_for_connector(Box::new(connector), aliases)
        }
        "sourcehut" => {
            let Some(tracker_id) = identity
                .tracker_id
                .as_ref()
                .filter(|value| !value.is_empty())
            else {
                return Ok(None);
            };
            let connector = SourcehutConnector::new(
                SourcehutConfig {
                    tracker_id: tracker_id.clone(),
                    api_base: config
                        .api_base
                        .clone()
                        .unwrap_or_else(|| "https://todo.sr.ht/query".to_string()),
                    git_remote: sourcehut_git_remote(config, identity),
                    credential_env: config.credential_env.clone(),
                    credential_command: non_empty_command(config),
                },
                SourcehutHttpTransport,
            );
            runtime_for_connector(Box::new(connector), aliases)
        }
        _ => Ok(None),
    }
}

fn runtime_for_connector(
    connector: Box<dyn ForgeConnector>,
    aliases: HashMap<String, String>,
) -> Result<Option<ForgeRuntime>, RuntimeError> {
    let tools = compose_tool_sets(&[connector.tool_set()], &aliases)
        .map_err(|error| RuntimeError::Composition(error.to_string()))?;
    Ok(Some(ForgeRuntime { tools, connector }))
}

fn non_empty_command(config: &ForgeConfig) -> Option<Vec<String>> {
    (!config.credential_command.is_empty()).then(|| config.credential_command.clone())
}

fn has_connector_identity_config(config: &ForgeConfig) -> bool {
    [
        config.forge_type.as_deref(),
        config.owner.as_deref(),
        config.name.as_deref(),
        config.tracker_id.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|value| !value.trim().is_empty())
}

fn sourcehut_git_remote(config: &ForgeConfig, identity: &ResolvedForgeIdentity) -> String {
    let remote = config.git_remote.clone().unwrap_or_default();
    let Some(owner) = identity.owner.as_deref().filter(|value| !value.is_empty()) else {
        return remote;
    };
    let Some(name) = identity.name.as_deref().filter(|value| !value.is_empty()) else {
        return remote;
    };
    if remote.trim().is_empty() {
        return remote;
    }
    remote_with_repo_coordinate(&remote, owner, name)
}

fn remote_with_repo_coordinate(remote: &str, owner: &str, name: &str) -> String {
    if let Some((prefix, path)) = split_url_remote(remote) {
        return format!("{prefix}{}", replace_repo_path(path, owner, name));
    }
    if let Some((prefix, path)) = split_scp_remote(remote) {
        return format!("{prefix}{}", replace_repo_path(path, owner, name));
    }
    replace_repo_path(remote, owner, name)
}

fn split_url_remote(remote: &str) -> Option<(&str, &str)> {
    let scheme = remote.find("://")?;
    let authority_start = scheme + 3;
    let path_offset = remote[authority_start..].find('/')?;
    let path_start = authority_start + path_offset + 1;
    Some(remote.split_at(path_start))
}

fn split_scp_remote(remote: &str) -> Option<(&str, &str)> {
    if remote.contains("://") {
        return None;
    }
    let colon = remote.find(':')?;
    let first_slash = remote.find('/');
    if first_slash.is_some_and(|slash| slash < colon) {
        return None;
    }
    Some(remote.split_at(colon + 1))
}

fn replace_repo_path(path: &str, owner: &str, name: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    let mut parts = trimmed.split('/').collect::<Vec<_>>();
    if parts.first().is_some_and(|part| part.starts_with('~')) {
        return format!("~{owner}/{name}");
    }
    match parts.len() {
        0 => format!("{owner}/{name}"),
        1 => format!("{owner}/{name}"),
        _ => {
            parts.truncate(parts.len() - 2);
            if parts.is_empty() {
                format!("{owner}/{name}")
            } else {
                format!("{}/{owner}/{name}", parts.join("/"))
            }
        }
    }
}

pub fn operation_for_tool(runtime: &ForgeRuntime, exposed_name: &str) -> Option<Operation> {
    runtime.tools.get(exposed_name).map(|tool| tool.operation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use runa_forge_contract::ForgeConnector;
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct RecordingConnector {
        calls: Arc<Mutex<Vec<Operation>>>,
    }

    impl ForgeConnector for RecordingConnector {
        fn set_name(&self) -> &str {
            "test"
        }

        fn call(&self, operation: Operation, _input: Value) -> Result<Value, ForgeError> {
            self.calls.lock().unwrap().push(operation);
            Ok(json!({ "ok": true }))
        }
    }

    #[test]
    fn runtime_rejects_missing_required_input_for_every_operation_before_connector_dispatch() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let runtime = runtime_for_connector(
            Box::new(RecordingConnector {
                calls: Arc::clone(&calls),
            }),
            HashMap::new(),
        )
        .unwrap()
        .expect("test connector should compose");

        for operation in Operation::ALL {
            let tool_name = operation.canonical_name();
            let required = runtime.tools[tool_name]
                .input_schema
                .get("required")
                .and_then(Value::as_array)
                .expect("operation schema should list required fields")
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .expect("required field should be a string")
                        .to_string()
                })
                .collect::<Vec<_>>();

            for missing_field in required {
                calls.lock().unwrap().clear();
                let mut input = valid_input(operation);
                input
                    .as_object_mut()
                    .expect("test input should be an object")
                    .remove(&missing_field);

                let error = runtime
                    .call_tool(tool_name, input)
                    .expect_err("missing required input should be rejected");

                assert!(
                    error.to_string().contains(&missing_field),
                    "{tool_name} error should name missing field {missing_field}: {error}"
                );
                assert!(
                    calls.lock().unwrap().is_empty(),
                    "{tool_name} dispatched to connector despite missing {missing_field}"
                );
            }
        }
    }

    fn valid_input(operation: Operation) -> Value {
        let work_unit = json!({
            "id": "test:scope:issue:203",
            "display": "scope#203"
        });
        let change = json!({
            "id": "test:scope:change:12:version:1",
            "display": "change#12"
        });
        let disposition = json!({
            "kind": "approved",
            "against_version": 1,
            "reviewer": "reviewer",
            "reviewed_at": "2026-06-17T00:00:00Z",
            "findings": []
        });
        let completion = json!({
            "criterion_summary": "done",
            "gaps": [],
            "change_reference": "abc123",
            "documentation_status": "updated"
        });

        match operation {
            Operation::ReadTicket => json!({ "reference": "203" }),
            Operation::CreateTicket => json!({ "title": "title", "body": "body" }),
            Operation::ClaimWorkUnit => json!({ "handle": work_unit }),
            Operation::RecordProgress => json!({ "handle": work_unit, "body": "progress" }),
            Operation::DeliverChangeProposal => json!({
                "work_unit": work_unit,
                "branch": "issue-203",
                "commit": "abc123",
                "base": "main",
                "summary": "summary",
                "body": "body",
                "version": 1
            }),
            Operation::ReflectDisposition => json!({
                "work_unit": work_unit,
                "change": change,
                "disposition": disposition,
                "body": "approved"
            }),
            Operation::ApplyApprovedChange => json!({
                "work_unit": work_unit,
                "change": change,
                "approved_version": 1,
                "approved_commit": "abc123",
                "base": "main"
            }),
            Operation::CloseOut => json!({
                "work_unit": work_unit,
                "completion": completion,
                "body": "done"
            }),
        }
    }
}
