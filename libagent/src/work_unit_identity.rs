use std::collections::BTreeMap;
use std::fmt;

use serde_json::Value;

use crate::store::{ArtifactStore, ValidationStatus};

#[derive(Debug)]
pub enum WorkUnitIdentityError {
    Io {
        instance_id: String,
        source: std::io::Error,
    },
    Json {
        instance_id: String,
        source: serde_json::Error,
    },
    HandleDisagreesWithInstanceId {
        instance_id: String,
        handle_number: u64,
    },
    DuplicateTicketRoots {
        handle_number: u64,
        instance_ids: Vec<String>,
    },
}

impl fmt::Display for WorkUnitIdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WorkUnitIdentityError::Io {
                instance_id,
                source,
            } => write!(
                f,
                "could not read work-unit artifact '{instance_id}' while validating work-unit identity: {source}"
            ),
            WorkUnitIdentityError::Json {
                instance_id,
                source,
            } => write!(
                f,
                "could not parse work-unit artifact '{instance_id}' while validating work-unit identity: {source}"
            ),
            WorkUnitIdentityError::HandleDisagreesWithInstanceId {
                instance_id,
                handle_number,
            } => write!(
                f,
                "work-unit identity conflict: instance id '{instance_id}' disagrees with handle ticket number {handle_number}"
            ),
            WorkUnitIdentityError::DuplicateTicketRoots {
                handle_number,
                instance_ids,
            } => write!(
                f,
                "work-unit identity conflict: ticket number {handle_number} has multiple delivered work-unit roots: {}",
                instance_ids.join(", ")
            ),
        }
    }
}

impl std::error::Error for WorkUnitIdentityError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WorkUnitIdentityError::Io { source, .. } => Some(source),
            WorkUnitIdentityError::Json { source, .. } => Some(source),
            WorkUnitIdentityError::HandleDisagreesWithInstanceId { .. }
            | WorkUnitIdentityError::DuplicateTicketRoots { .. } => None,
        }
    }
}

#[derive(Debug, Clone)]
struct TicketBackedWorkUnit {
    instance_id: String,
    number: u64,
}

pub fn validate_scoped_work_unit_identity(
    store: &ArtifactStore,
    work_unit: &str,
) -> Result<(), WorkUnitIdentityError> {
    let mut ticket_backed = Vec::new();
    for (instance_id, state) in store.instances_of("work-unit", None) {
        if !matches!(state.status, ValidationStatus::Valid) {
            continue;
        }
        let content =
            std::fs::read_to_string(&state.path).map_err(|source| WorkUnitIdentityError::Io {
                instance_id: instance_id.to_string(),
                source,
            })?;
        let value: Value =
            serde_json::from_str(&content).map_err(|source| WorkUnitIdentityError::Json {
                instance_id: instance_id.to_string(),
                source,
            })?;
        let Some(number) = handle_number(&value) else {
            continue;
        };
        ticket_backed.push(TicketBackedWorkUnit {
            instance_id: instance_id.to_string(),
            number,
        });
    }

    let Some(selected) = ticket_backed
        .iter()
        .find(|candidate| candidate.instance_id == work_unit)
    else {
        return Ok(());
    };
    let selected_instance_id = selected.instance_id.clone();
    let selected_number = selected.number;

    if instance_ticket_number(&selected_instance_id) != Some(selected_number) {
        return Err(WorkUnitIdentityError::HandleDisagreesWithInstanceId {
            instance_id: selected_instance_id,
            handle_number: selected_number,
        });
    }

    let mut by_number: BTreeMap<u64, Vec<String>> = BTreeMap::new();
    for candidate in ticket_backed {
        by_number
            .entry(candidate.number)
            .or_default()
            .push(candidate.instance_id);
    }
    if let Some(mut instance_ids) = by_number.remove(&selected_number)
        && instance_ids.len() > 1
    {
        instance_ids.sort();
        return Err(WorkUnitIdentityError::DuplicateTicketRoots {
            handle_number: selected_number,
            instance_ids,
        });
    }

    Ok(())
}

fn handle_number(value: &Value) -> Option<u64> {
    value
        .get("handle")
        .and_then(|handle| handle.get("number"))
        .and_then(Value::as_u64)
}

fn instance_ticket_number(instance_id: &str) -> Option<u64> {
    let rest = instance_id.strip_prefix("work-unit-")?;
    let digits: String = rest
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return None;
    }
    let after_digits = &rest[digits.len()..];
    if !after_digits.is_empty() && !after_digits.starts_with('-') {
        return None;
    }
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::instance_ticket_number;

    #[test]
    fn instance_ticket_number_extracts_conventional_work_unit_number() {
        assert_eq!(
            Some(363),
            instance_ticket_number("work-unit-363-ticket-handle")
        );
        assert_eq!(None, instance_ticket_number("issue-363-ticket-handle"));
        assert_eq!(None, instance_ticket_number("work-unit-ticket-handle"));
    }
}
