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
        self.connector
            .call(tool.operation, input)
            .map_err(RuntimeError::Forge)
    }
}

#[derive(Debug)]
pub enum RuntimeError {
    UnknownTool(String),
    Composition(String),
    Forge(ForgeError),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuntimeError::UnknownTool(tool) => write!(f, "unknown forge tool '{tool}'"),
            RuntimeError::Composition(message) => write!(f, "{message}"),
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
