use libagent::ForgeConfig;
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
    let forge_type = config.forge_type.as_deref().unwrap_or("github");
    let aliases: HashMap<String, String> = config
        .tool_aliases
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();

    match forge_type {
        "github" => {
            let Some(owner) = config.owner.as_ref().filter(|value| !value.is_empty()) else {
                return Ok(None);
            };
            let Some(repo) = config.name.as_ref().filter(|value| !value.is_empty()) else {
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
                    credential_env: config.credential_env.clone(),
                    credential_command: non_empty_command(config),
                },
                GithubHttpTransport,
            );
            runtime_for_connector(Box::new(connector), aliases)
        }
        "sourcehut" => {
            let Some(tracker_id) = config.tracker_id.as_ref().filter(|value| !value.is_empty())
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
                    git_remote: config.git_remote.clone().unwrap_or_default(),
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

pub fn operation_for_tool(runtime: &ForgeRuntime, exposed_name: &str) -> Option<Operation> {
    runtime.tools.get(exposed_name).map(|tool| tool.operation)
}
