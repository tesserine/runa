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
    NonExactTrackerWorkUnitScope {
        supplied_work_unit: String,
        canonical_instance_ids: Vec<String>,
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
            WorkUnitIdentityError::NonExactTrackerWorkUnitScope {
                supplied_work_unit,
                canonical_instance_ids,
            } => write!(
                f,
                "work-unit identity conflict: scoped work-unit '{supplied_work_unit}' does not exactly match a delivered tracker-backed work-unit root; available canonical id(s): {}",
                canonical_instance_ids.join(", ")
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
            | WorkUnitIdentityError::NonExactTrackerWorkUnitScope { .. }
            | WorkUnitIdentityError::DuplicateTicketRoots { .. } => None,
        }
    }
}

#[derive(Debug, Clone)]
struct TicketBackedWorkUnit {
    instance_id: String,
    identity: TicketIdentity,
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct TicketIdentity {
    forge_tag: String,
    sourcehut_tracker_id: Option<u64>,
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
        let Some(identity) = handle_identity(&value) else {
            continue;
        };
        ticket_backed.push(TicketBackedWorkUnit {
            instance_id: instance_id.to_string(),
            identity,
        });
    }

    let Some(selected) = ticket_backed
        .iter()
        .find(|candidate| candidate.instance_id == work_unit)
    else {
        // Numeric and conventional-prefix aliases are tracker intent when they
        // resolve to a delivered ticket-backed root. They must not bypass the
        // exact artifact id used to thread scoped execution.
        if let Some(supplied_number) = scope_ticket_number(work_unit) {
            let mut canonical_instance_ids: Vec<String> = ticket_backed
                .iter()
                .filter(|candidate| candidate.identity.number == supplied_number)
                .map(|candidate| candidate.instance_id.clone())
                .collect();
            canonical_instance_ids.sort();
            if !canonical_instance_ids.is_empty() {
                return Err(WorkUnitIdentityError::NonExactTrackerWorkUnitScope {
                    supplied_work_unit: work_unit.to_string(),
                    canonical_instance_ids,
                });
            }
        }
        return Ok(());
    };
    let selected_instance_id = selected.instance_id.clone();
    let selected_identity = selected.identity.clone();

    if instance_ticket_number(&selected_instance_id) != Some(selected_identity.number) {
        return Err(WorkUnitIdentityError::HandleDisagreesWithInstanceId {
            instance_id: selected_instance_id,
            handle_number: selected_identity.number,
        });
    }

    let mut by_ticket_identity: BTreeMap<TicketIdentity, Vec<String>> = BTreeMap::new();
    for candidate in ticket_backed {
        // Duplicate roots are only conflicts for the same forge ticket. The
        // numeric id alone is not unique across forges or SourceHut trackers.
        by_ticket_identity
            .entry(candidate.identity)
            .or_default()
            .push(candidate.instance_id);
    }
    if let Some(mut instance_ids) = by_ticket_identity.remove(&selected_identity)
        && instance_ids.len() > 1
    {
        instance_ids.sort();
        return Err(WorkUnitIdentityError::DuplicateTicketRoots {
            handle_number: selected_identity.number,
            instance_ids,
        });
    }

    Ok(())
}

fn handle_identity(value: &Value) -> Option<TicketIdentity> {
    let handle = value.get("handle")?;
    let forge_tag = handle
        .get("forge_tag")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let sourcehut_tracker_id = if forge_tag == "sourcehut" {
        handle.get("tracker_id").and_then(Value::as_u64)
    } else {
        None
    };
    Some(TicketIdentity {
        forge_tag,
        sourcehut_tracker_id,
        number: handle.get("number")?.as_u64()?,
    })
}

fn scope_ticket_number(work_unit: &str) -> Option<u64> {
    if work_unit
        .chars()
        .all(|character| character.is_ascii_digit())
    {
        return work_unit.parse().ok();
    }
    instance_ticket_number(work_unit)
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
    use super::{
        WorkUnitIdentityError, instance_ticket_number, validate_scoped_work_unit_identity,
    };
    use crate::store::ArtifactStore;
    use crate::test_helpers::{make_artifact_type, simple_schema};
    use serde_json::{Value, json};
    use std::path::Path;
    use tempfile::TempDir;

    fn test_store(tmp: &TempDir) -> ArtifactStore {
        ArtifactStore::new(
            vec![make_artifact_type("work-unit", simple_schema())],
            tmp.path().join("store"),
        )
        .unwrap()
    }

    fn record_work_unit(
        store: &mut ArtifactStore,
        tmp: &TempDir,
        instance_id: &str,
        handle: Option<Value>,
    ) {
        let mut data = json!({"title": "Ticket-backed work unit"});
        if let Some(handle) = handle {
            data.as_object_mut()
                .unwrap()
                .insert("handle".to_string(), handle);
        }
        let path = tmp.path().join(format!("{instance_id}.json"));
        std::fs::write(&path, serde_json::to_string(&data).unwrap()).unwrap();
        store
            .record("work-unit", instance_id, Path::new(&path), &data)
            .unwrap();
    }

    fn github_handle(number: u64) -> Value {
        json!({
            "forge_tag": "github",
            "url": format!("https://github.com/tesserine/groundwork/issues/{number}"),
            "number": number
        })
    }

    fn sourcehut_handle(tracker_id: u64, number: u64) -> Value {
        json!({
            "forge_tag": "sourcehut",
            "tracker_id": tracker_id,
            "number": number
        })
    }

    #[test]
    fn instance_ticket_number_extracts_conventional_work_unit_number() {
        assert_eq!(
            Some(363),
            instance_ticket_number("work-unit-363-ticket-handle")
        );
        assert_eq!(None, instance_ticket_number("issue-363-ticket-handle"));
        assert_eq!(None, instance_ticket_number("work-unit-ticket-handle"));
    }

    #[test]
    fn non_exact_tracker_scopes_are_rejected_with_the_available_canonical_id() {
        let tmp = TempDir::new().unwrap();
        let mut store = test_store(&tmp);
        record_work_unit(
            &mut store,
            &tmp,
            "work-unit-363-ticket-handle",
            Some(github_handle(363)),
        );

        for supplied in ["work-unit-363", "363"] {
            let error = validate_scoped_work_unit_identity(&store, supplied).unwrap_err();
            let rendered = error.to_string();

            assert!(rendered.contains(supplied), "error: {rendered}");
            assert!(
                rendered.contains("work-unit-363-ticket-handle"),
                "error: {rendered}"
            );
        }
    }

    #[test]
    fn exact_delivered_tracker_scope_is_accepted() {
        let tmp = TempDir::new().unwrap();
        let mut store = test_store(&tmp);
        record_work_unit(
            &mut store,
            &tmp,
            "work-unit-363-ticket-handle",
            Some(github_handle(363)),
        );

        validate_scoped_work_unit_identity(&store, "work-unit-363-ticket-handle").unwrap();
    }

    #[test]
    fn true_non_tracker_scope_keeps_pass_through_behavior() {
        let tmp = TempDir::new().unwrap();
        let mut store = test_store(&tmp);
        record_work_unit(&mut store, &tmp, "work-unit-freeform", None);

        validate_scoped_work_unit_identity(&store, "work-unit-freeform").unwrap();
        validate_scoped_work_unit_identity(&store, "work-unit-missing").unwrap();
    }

    #[test]
    fn same_number_on_different_forges_is_not_a_duplicate_ticket_root() {
        let tmp = TempDir::new().unwrap();
        let mut store = test_store(&tmp);
        record_work_unit(
            &mut store,
            &tmp,
            "work-unit-5-github-ticket",
            Some(github_handle(5)),
        );
        record_work_unit(
            &mut store,
            &tmp,
            "work-unit-5-sourcehut-ticket",
            Some(sourcehut_handle(10, 5)),
        );

        validate_scoped_work_unit_identity(&store, "work-unit-5-github-ticket").unwrap();
        validate_scoped_work_unit_identity(&store, "work-unit-5-sourcehut-ticket").unwrap();
    }

    #[test]
    fn same_number_on_different_sourcehut_trackers_is_not_a_duplicate_ticket_root() {
        let tmp = TempDir::new().unwrap();
        let mut store = test_store(&tmp);
        record_work_unit(
            &mut store,
            &tmp,
            "work-unit-5-sourcehut-tracker-10-ticket",
            Some(sourcehut_handle(10, 5)),
        );
        record_work_unit(
            &mut store,
            &tmp,
            "work-unit-5-sourcehut-tracker-11-ticket",
            Some(sourcehut_handle(11, 5)),
        );

        validate_scoped_work_unit_identity(&store, "work-unit-5-sourcehut-tracker-10-ticket")
            .unwrap();
        validate_scoped_work_unit_identity(&store, "work-unit-5-sourcehut-tracker-11-ticket")
            .unwrap();
    }

    #[test]
    fn divergent_delivered_ids_for_one_full_ticket_identity_are_rejected() {
        let tmp = TempDir::new().unwrap();
        let mut store = test_store(&tmp);
        record_work_unit(
            &mut store,
            &tmp,
            "work-unit-5-sourcehut-ticket",
            Some(sourcehut_handle(10, 5)),
        );
        record_work_unit(
            &mut store,
            &tmp,
            "work-unit-5-renamed-sourcehut-ticket",
            Some(sourcehut_handle(10, 5)),
        );

        let error =
            validate_scoped_work_unit_identity(&store, "work-unit-5-sourcehut-ticket").unwrap_err();

        assert!(matches!(
            error,
            WorkUnitIdentityError::DuplicateTicketRoots { .. }
        ));
    }
}
