//! GitHub forge connector.

use runa_forge::{
    ConnectorConfig, ForgeConnector, ForgeError, Handle, Operation, handle_value, require_string,
};
use serde_json::{Value, json};

pub struct GithubConnector {
    config: ConnectorConfig,
    owner: String,
    name: String,
}

impl GithubConnector {
    pub fn new(config: ConnectorConfig) -> Result<Self, ForgeError> {
        if config.provider != "github" {
            return Err(ForgeError::Config(format!(
                "github connector cannot use provider '{}'",
                config.provider
            )));
        }
        let owner = config.required_owner()?.to_string();
        let name = config.required_name()?.to_string();
        Ok(Self {
            config,
            owner,
            name,
        })
    }

    fn parse_ticket_reference(&self, reference: &str) -> Result<u64, ForgeError> {
        let trimmed = reference.trim();
        let scoped = if let Some(rest) = trimmed.strip_prefix("github:") {
            Some(rest)
        } else if trimmed.contains('/') {
            Some(trimmed)
        } else {
            None
        };

        if let Some(scoped) = scoped {
            let (repository, number) = scoped.split_once('#').ok_or_else(|| {
                ForgeError::InvalidInput("ticket reference must contain '#N'".to_string())
            })?;
            let expected = format!("{}/{}", self.owner, self.name);
            if repository != expected {
                return Err(ForgeError::ForeignScope(format!(
                    "reference names github:{repository}, connector is configured for github:{expected}"
                )));
            }
            return parse_number(number);
        }

        let number = trimmed.strip_prefix('#').unwrap_or(trimmed);
        parse_number(number)
    }

    fn work_unit_handle(&self, number: u64) -> Handle {
        Handle {
            id: format!("github:{}/{}:issue:{number}", self.owner, self.name),
            display: format!("github:{}/{}#{number}", self.owner, self.name),
        }
    }

    fn validate_work_unit_handle(&self, handle: &Value) -> Result<Handle, ForgeError> {
        let id = require_string(handle, "id")?;
        let display = require_string(handle, "display")?;
        let prefix = format!("github:{}/{}:issue:", self.owner, self.name);
        if !id.starts_with(&prefix) {
            return Err(ForgeError::ForeignScope(format!(
                "handle id '{id}' is outside github:{}/{}",
                self.owner, self.name
            )));
        }
        Ok(Handle {
            id: id.to_string(),
            display: display.to_string(),
        })
    }
}

impl ForgeConnector for GithubConnector {
    fn provider(&self) -> &'static str {
        "github"
    }

    fn dry_run(&self, operation: Operation, input: Value) -> Result<Value, ForgeError> {
        let _ = &self.config;
        match operation {
            Operation::ReadTicket => {
                let reference = require_string(&input, "reference")?;
                let number = self.parse_ticket_reference(reference)?;
                Ok(json!({
                    "handle": handle_value(self.work_unit_handle(number)),
                    "title": format!("ticket {number}"),
                    "body": null,
                    "state": "dry-run"
                }))
            }
            Operation::ClaimWorkUnit => {
                let handle = self.validate_work_unit_handle(&input["handle"])?;
                Ok(json!({
                    "handle": handle_value(handle),
                    "receipt": "dry-run: claim-work-unit"
                }))
            }
            Operation::RecordProgress => {
                let handle = self.validate_work_unit_handle(&input["handle"])?;
                Ok(json!({
                    "handle": handle_value(handle),
                    "receipt": "dry-run: record-progress"
                }))
            }
            other => Err(ForgeError::UnsupportedOperation(other)),
        }
    }
}

fn parse_number(value: &str) -> Result<u64, ForgeError> {
    value
        .parse::<u64>()
        .ok()
        .filter(|number| *number > 0)
        .ok_or_else(|| ForgeError::InvalidInput(format!("invalid ticket number '{value}'")))
}
