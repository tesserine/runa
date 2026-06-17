//! Deployment connector registry for runa.

use libagent::project::{
    ConnectorCredentialConfig, ForgeConnectorsConfig, GithubForgeConnectorConfig,
    SourcehutForgeConnectorConfig,
};
use runa_forge::{ConnectorConfig, CredentialSource, ForgeConnector, ForgeError};
use runa_forge_github::GithubConnector;
use runa_forge_sourcehut::SourcehutConnector;

pub type DynForgeConnector = Box<dyn ForgeConnector>;

pub fn configured_forge_connector(
    config: &libagent::project::Config,
) -> Result<Option<DynForgeConnector>, ForgeError> {
    forge_connector_from_config(&config.connectors.forge)
}

pub fn forge_connector_from_config(
    config: &ForgeConnectorsConfig,
) -> Result<Option<DynForgeConnector>, ForgeError> {
    let Some(provider) = config.provider.as_deref().filter(|value| !value.is_empty()) else {
        return Ok(None);
    };

    match provider {
        "github" => {
            let github = config.github.as_ref().ok_or_else(|| {
                ForgeError::Config("connectors.forge.github is required".to_string())
            })?;
            Ok(Some(Box::new(GithubConnector::new(github_config(
                github,
            )?)?)))
        }
        "sourcehut" => {
            let sourcehut = config.sourcehut.as_ref().ok_or_else(|| {
                ForgeError::Config("connectors.forge.sourcehut is required".to_string())
            })?;
            Ok(Some(Box::new(SourcehutConnector::new(sourcehut_config(
                sourcehut,
            )?)?)))
        }
        other => Err(ForgeError::Config(format!(
            "unsupported forge connector provider '{other}'"
        ))),
    }
}

fn github_config(config: &GithubForgeConnectorConfig) -> Result<ConnectorConfig, ForgeError> {
    let owner = required(config.owner.as_deref(), "connectors.forge.github.owner")?;
    let name = required(config.name.as_deref(), "connectors.forge.github.name")?;
    let mut connector =
        ConnectorConfig::github(owner, name, credential_source(config.credential.as_ref())?);
    if let Some(value) = config.api_base_url.as_ref() {
        connector
            .extra
            .insert("api_base_url".to_string(), value.clone());
    }
    if let Some(value) = config.web_base_url.as_ref() {
        connector
            .extra
            .insert("web_base_url".to_string(), value.clone());
    }
    if let Some(value) = config.git_remote.as_ref() {
        connector
            .extra
            .insert("git_remote".to_string(), value.clone());
    }
    Ok(connector)
}

fn sourcehut_config(config: &SourcehutForgeConnectorConfig) -> Result<ConnectorConfig, ForgeError> {
    let owner = required(config.owner.as_deref(), "connectors.forge.sourcehut.owner")?;
    let name = required(config.name.as_deref(), "connectors.forge.sourcehut.name")?;
    let tracker_id = config.tracker_id.ok_or_else(|| {
        ForgeError::Config("connectors.forge.sourcehut.tracker_id is required".to_string())
    })?;
    let endpoint = required(
        config.endpoint.as_deref(),
        "connectors.forge.sourcehut.endpoint",
    )?;
    let mut connector = ConnectorConfig::sourcehut(
        owner,
        name,
        tracker_id,
        endpoint,
        credential_source(config.credential.as_ref())?,
    );
    if let Some(value) = config.assignee_user_id {
        connector
            .extra
            .insert("assignee_user_id".to_string(), value.to_string());
    }
    Ok(connector)
}

fn credential_source(
    config: Option<&ConnectorCredentialConfig>,
) -> Result<CredentialSource, ForgeError> {
    let Some(config) = config else {
        return Ok(CredentialSource::None);
    };
    match (config.env.as_ref(), config.command.as_ref()) {
        (Some(env), None) if !env.is_empty() => Ok(CredentialSource::Env(env.clone())),
        (None, Some(command)) if !command.is_empty() => {
            Ok(CredentialSource::Command(command.clone()))
        }
        (None, None) => Ok(CredentialSource::None),
        _ => Err(ForgeError::Config(
            "connector credential must specify exactly one non-empty env or command".to_string(),
        )),
    }
}

fn required<'a>(value: Option<&'a str>, name: &str) -> Result<&'a str, ForgeError> {
    value
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ForgeError::Config(format!("{name} is required")))
}
