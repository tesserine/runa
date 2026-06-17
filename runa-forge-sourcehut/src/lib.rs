//! SourceHut forge connector.

use runa_forge::{
    ConnectorConfig, ForgeConnector, ForgeError, Handle, Operation, handle_value, require_string,
};
use serde_json::{Value, json};

pub struct SourcehutConnector {
    config: ConnectorConfig,
    tracker_id: u64,
}

impl SourcehutConnector {
    pub fn new(config: ConnectorConfig) -> Result<Self, ForgeError> {
        if config.provider != "sourcehut" {
            return Err(ForgeError::Config(format!(
                "sourcehut connector cannot use provider '{}'",
                config.provider
            )));
        }
        let tracker_id = config.required_tracker_id()?;
        Ok(Self { config, tracker_id })
    }

    fn parse_ticket_reference(&self, reference: &str) -> Result<u64, ForgeError> {
        let trimmed = reference.trim();
        if let Some(rest) = trimmed.strip_prefix("sourcehut:") {
            let (tracker, number) = rest.split_once('#').ok_or_else(|| {
                ForgeError::InvalidInput("ticket reference must contain '#N'".to_string())
            })?;
            let tracker = parse_number(tracker)?;
            if tracker != self.tracker_id {
                return Err(ForgeError::ForeignScope(format!(
                    "reference names sourcehut:{tracker}, connector is configured for sourcehut:{}",
                    self.tracker_id
                )));
            }
            return parse_number(number);
        }

        let number = trimmed.strip_prefix('#').unwrap_or(trimmed);
        parse_number(number)
    }

    fn work_unit_handle(&self, number: u64) -> Handle {
        Handle {
            id: format!("sourcehut:tracker:{}:ticket:{number}", self.tracker_id),
            display: format!("sourcehut:{}#{number}", self.tracker_id),
        }
    }

    fn validate_work_unit_handle(&self, handle: &Value) -> Result<Handle, ForgeError> {
        let id = require_string(handle, "id")?;
        let display = require_string(handle, "display")?;
        let prefix = format!("sourcehut:tracker:{}:ticket:", self.tracker_id);
        if !id.starts_with(&prefix) {
            return Err(ForgeError::ForeignScope(format!(
                "handle id '{id}' is outside sourcehut:{}",
                self.tracker_id
            )));
        }
        Ok(Handle {
            id: id.to_string(),
            display: display.to_string(),
        })
    }
}

impl ForgeConnector for SourcehutConnector {
    fn provider(&self) -> &'static str {
        "sourcehut"
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
