use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use runa_forge_contract::{
    ForgeConnectorConfig, ForgeConnectorsConfig, ForgeOperation, ForgeTool, ForgeToolSet,
    ToolAliases, compose_tool_sets,
};
use runa_forge_github::{GitHubConfig, GitHubConnector, RecordingGitHubTransport};
use runa_forge_sourcehut::{RecordingSourceHutTransport, SourceHutConfig, SourceHutConnector};
use serde_json::Value;

#[derive(Clone, Debug, Default)]
pub struct ConfiguredForgeConnectors {
    connectors: Vec<ConfiguredConnector>,
}

impl ConfiguredForgeConnectors {
    pub fn from_config(config: &ForgeConnectorsConfig) -> Result<Self, ForgeConnectorConfigError> {
        let mut labels = BTreeSet::new();
        let mut connectors = Vec::new();
        for connector in &config.forge {
            let configured = ConfiguredConnector::from_config(connector)?;
            if !labels.insert(configured.label.clone()) {
                return Err(ForgeConnectorConfigError::DuplicateLabel(
                    configured.label.clone(),
                ));
            }
            connectors.push(configured);
        }

        let registry = Self { connectors };
        registry.composed_tools()?;
        Ok(registry)
    }

    pub fn is_empty(&self) -> bool {
        self.connectors.is_empty()
    }

    pub fn composed_tools(&self) -> Result<Vec<ForgeTool>, ForgeConnectorConfigError> {
        let aliases = self.aliases();
        let sets = self
            .connectors
            .iter()
            .map(ConfiguredConnector::tool_set)
            .collect::<Vec<_>>();
        compose_tool_sets(sets, aliases)
            .map_err(|error| ForgeConnectorConfigError::CompositionFailed(error.to_string()))
    }

    pub fn contains_tool(&self, tool_name: &str) -> Result<bool, ForgeConnectorConfigError> {
        Ok(self
            .composed_tools()?
            .iter()
            .any(|tool| tool.name == tool_name))
    }

    pub fn call(&self, tool_name: &str, input: Value) -> Result<Value, ForgeConnectorCallError> {
        let aliases = self.aliases().aliases;
        for connector in &self.connectors {
            let set = connector.tool_set();
            for tool in &set.tools {
                let name = aliases
                    .get(&(set.provider.clone(), tool.name.clone()))
                    .cloned()
                    .unwrap_or_else(|| tool.name.clone());
                if name == tool_name {
                    return connector.call(tool.operation, input);
                }
            }
        }
        Err(ForgeConnectorCallError::UnknownTool(tool_name.to_owned()))
    }

    fn aliases(&self) -> ToolAliases {
        let mut aliases = ToolAliases::default();
        for connector in &self.connectors {
            if let Some(prefix) = &connector.alias_prefix {
                for operation in ForgeOperation::ALL {
                    aliases.insert(
                        connector.label.clone(),
                        operation.name(),
                        format!("{prefix}.{}", operation.name()),
                    );
                }
            }
        }
        aliases
    }
}

#[derive(Clone, Debug)]
enum ConfiguredConnectorKind {
    GitHub(GitHubConnector),
    SourceHut(SourceHutConnector),
}

#[derive(Clone, Debug)]
struct ConfiguredConnector {
    label: String,
    alias_prefix: Option<String>,
    kind: ConfiguredConnectorKind,
}

impl ConfiguredConnector {
    fn from_config(config: &ForgeConnectorConfig) -> Result<Self, ForgeConnectorConfigError> {
        let label = config
            .label
            .clone()
            .unwrap_or_else(|| config.provider.clone());
        let kind = match config.provider.as_str() {
            "github" => {
                let mut github = GitHubConfig::new(config.owner.clone(), config.name.clone());
                github.credential_env = config.credentials.env.clone();
                github.credential_command = config.credentials.command.clone();
                ConfiguredConnectorKind::GitHub(GitHubConnector::new_for_test(
                    github,
                    RecordingGitHubTransport::default(),
                ))
            }
            "sourcehut" => {
                let tracker_id =
                    config
                        .tracker_id
                        .ok_or_else(|| ForgeConnectorConfigError::MissingField {
                            provider: config.provider.clone(),
                            field: "tracker_id",
                        })?;
                let endpoint = config.endpoint.clone().ok_or_else(|| {
                    ForgeConnectorConfigError::MissingField {
                        provider: config.provider.clone(),
                        field: "endpoint",
                    }
                })?;
                let repo_id =
                    config
                        .repo_id
                        .ok_or_else(|| ForgeConnectorConfigError::MissingField {
                            provider: config.provider.clone(),
                            field: "repo_id",
                        })?;
                let mut sourcehut = SourceHutConfig::new(
                    config.owner.clone(),
                    config.name.clone(),
                    tracker_id,
                    endpoint,
                    repo_id,
                );
                sourcehut.credential_env = config.credentials.env.clone();
                sourcehut.credential_command = config.credentials.command.clone();
                ConfiguredConnectorKind::SourceHut(SourceHutConnector::new_for_test(
                    sourcehut,
                    RecordingSourceHutTransport::default(),
                ))
            }
            other => return Err(ForgeConnectorConfigError::UnknownProvider(other.to_owned())),
        };

        Ok(Self {
            label,
            alias_prefix: config.alias_prefix.clone(),
            kind,
        })
    }

    fn tool_set(&self) -> ForgeToolSet {
        let mut set = match &self.kind {
            ConfiguredConnectorKind::GitHub(connector) => connector.tool_set(),
            ConfiguredConnectorKind::SourceHut(connector) => connector.tool_set(),
        };
        set.provider = self.label.clone();
        set
    }

    fn call(
        &self,
        operation: ForgeOperation,
        input: Value,
    ) -> Result<Value, ForgeConnectorCallError> {
        match &self.kind {
            ConfiguredConnectorKind::GitHub(connector) => connector
                .call(operation, input)
                .map_err(|error| ForgeConnectorCallError::Provider(error.to_string())),
            ConfiguredConnectorKind::SourceHut(connector) => connector
                .call(operation, input)
                .map_err(|error| ForgeConnectorCallError::Provider(error.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForgeConnectorConfigError {
    UnknownProvider(String),
    MissingField {
        provider: String,
        field: &'static str,
    },
    DuplicateLabel(String),
    CompositionFailed(String),
}

impl fmt::Display for ForgeConnectorConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ForgeConnectorConfigError::UnknownProvider(provider) => {
                write!(f, "unknown forge connector provider `{provider}`")
            }
            ForgeConnectorConfigError::MissingField { provider, field } => {
                write!(
                    f,
                    "forge connector `{provider}` is missing required field `{field}`"
                )
            }
            ForgeConnectorConfigError::DuplicateLabel(label) => {
                write!(f, "duplicate forge connector label `{label}`")
            }
            ForgeConnectorConfigError::CompositionFailed(error) => f.write_str(error),
        }
    }
}

impl std::error::Error for ForgeConnectorConfigError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForgeConnectorCallError {
    UnknownTool(String),
    Provider(String),
}

impl fmt::Display for ForgeConnectorCallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ForgeConnectorCallError::UnknownTool(tool) => {
                write!(f, "unknown forge connector tool `{tool}`")
            }
            ForgeConnectorCallError::Provider(error) => f.write_str(error),
        }
    }
}

impl std::error::Error for ForgeConnectorCallError {}

pub fn tool_by_name(tools: Vec<ForgeTool>) -> BTreeMap<String, ForgeTool> {
    tools
        .into_iter()
        .map(|tool| (tool.name.clone(), tool))
        .collect()
}
