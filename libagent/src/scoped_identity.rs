//! Canonical scoped work-unit identity validation.
//!
//! Runa owns scope identity. For tracker-backed work, a `work-unit` handle
//! carries the full tracker identity generated from the configured
//! forge-address contract; this module verifies recorded roots against that
//! configured address set.

use std::collections::HashMap;
use std::fmt;

use serde_json::Value;

use crate::forge_address::{
    ForgeAddressError, ForgeProject, tracker_identity_from_handle, work_unit_identity_from_handle,
};
use crate::store::{ArtifactStore, ValidationStatus};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopedWorkUnitError {
    NonCanonical {
        supplied: String,
        available: Vec<String>,
    },
    TrackerNumberParseFailure {
        instance_id: String,
    },
    TrackerNumberDisagreement {
        instance_id: String,
        handle_number: u64,
    },
    DuplicateTrackerRoots {
        first_instance_id: String,
        duplicate_instance_id: String,
        tracker_identity: String,
    },
    DeploymentDisagreement {
        instance_id: String,
        handle_identity: String,
        configured_identities: Vec<String>,
    },
    ForgeAddress(ForgeAddressError),
    WorkUnitScanIncomplete,
}

impl fmt::Display for ScopedWorkUnitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScopedWorkUnitError::NonCanonical {
                supplied,
                available,
            } => write!(
                f,
                "scoped work unit '{supplied}' is not a recorded canonical work-unit id; available canonical work-unit ids: {}",
                available.join(", ")
            ),
            ScopedWorkUnitError::TrackerNumberParseFailure { instance_id } => write!(
                f,
                "work-unit instance id '{instance_id}' does not contain a parseable tracker number"
            ),
            ScopedWorkUnitError::TrackerNumberDisagreement {
                instance_id,
                handle_number,
            } => write!(
                f,
                "work-unit instance id '{instance_id}' disagrees with tracker handle number {handle_number}"
            ),
            ScopedWorkUnitError::DuplicateTrackerRoots {
                first_instance_id,
                duplicate_instance_id,
                tracker_identity,
            } => write!(
                f,
                "work-unit instances '{first_instance_id}' and '{duplicate_instance_id}' share tracker identity {tracker_identity}"
            ),
            ScopedWorkUnitError::DeploymentDisagreement {
                instance_id,
                handle_identity,
                configured_identities,
            } => write!(
                f,
                "work-unit instance '{instance_id}' belongs to {handle_identity}, which does not resolve to a configured tracker identity (configured: {})",
                configured_identities.join(", ")
            ),
            ScopedWorkUnitError::ForgeAddress(error) => write!(f, "{error}"),
            ScopedWorkUnitError::WorkUnitScanIncomplete => write!(
                f,
                "the 'work-unit' artifact type was only partially scanned; the ticket cannot be resolved without complete work-unit scan trust"
            ),
        }
    }
}

impl std::error::Error for ScopedWorkUnitError {}

impl From<ForgeAddressError> for ScopedWorkUnitError {
    fn from(error: ForgeAddressError) -> Self {
        Self::ForgeAddress(error)
    }
}

pub fn validate_scoped_work_unit(
    store: &ArtifactStore,
    supplied: &str,
) -> Result<(), ScopedWorkUnitError> {
    validate_scoped_work_unit_with_project(store, supplied, &ForgeProject::default())
}

pub fn validate_tracker_consistency(
    store: &ArtifactStore,
    project: &ForgeProject,
) -> Result<(), ScopedWorkUnitError> {
    validate_tracker_content(store, project)
}

pub fn find_work_unit_by_tracker_identity(store: &ArtifactStore, target: &str) -> Option<String> {
    for (instance_id, state) in store.instances_of("work-unit", None) {
        if !matches!(state.status, ValidationStatus::Valid) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&state.path) else {
            continue;
        };
        let Ok(data) = serde_json::from_str::<Value>(&content) else {
            continue;
        };
        let Some(handle) = data.get("handle").and_then(Value::as_object) else {
            continue;
        };
        if work_unit_identity_from_handle(handle)
            .ok()
            .flatten()
            .as_deref()
            == Some(target)
        {
            return Some(instance_id.to_string());
        }
    }
    None
}

pub fn validate_scoped_work_unit_with_project(
    store: &ArtifactStore,
    supplied: &str,
    project: &ForgeProject,
) -> Result<(), ScopedWorkUnitError> {
    let available: Vec<String> = store
        .instances_of("work-unit", None)
        .into_iter()
        .map(|(instance_id, _)| instance_id.to_string())
        .collect();

    if available.is_empty() || available.iter().any(|id| id == supplied) {
        return validate_tracker_content(store, project);
    }

    Err(ScopedWorkUnitError::NonCanonical {
        supplied: supplied.to_string(),
        available,
    })
}

fn validate_tracker_content(
    store: &ArtifactStore,
    project: &ForgeProject,
) -> Result<(), ScopedWorkUnitError> {
    let mut tracker_roots = HashMap::new();

    for (instance_id, state) in store.instances_of("work-unit", None) {
        if !matches!(state.status, ValidationStatus::Valid) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&state.path) else {
            continue;
        };
        let Ok(data) = serde_json::from_str::<Value>(&content) else {
            continue;
        };
        let Some(handle) = data.get("handle").and_then(Value::as_object) else {
            continue;
        };
        let Some(handle_number) = handle.get("number").and_then(Value::as_u64) else {
            continue;
        };
        let instance_number = instance_work_unit_number(instance_id).ok_or_else(|| {
            ScopedWorkUnitError::TrackerNumberParseFailure {
                instance_id: instance_id.to_string(),
            }
        })?;
        if instance_number != handle_number {
            return Err(ScopedWorkUnitError::TrackerNumberDisagreement {
                instance_id: instance_id.to_string(),
                handle_number,
            });
        }

        let Some(tracker_identity) = tracker_identity_from_handle(handle)? else {
            continue;
        };
        let Some(work_unit_identity) = work_unit_identity_from_handle(handle)? else {
            continue;
        };
        if let Some(first_instance_id) =
            tracker_roots.insert(work_unit_identity.clone(), instance_id.to_string())
        {
            return Err(ScopedWorkUnitError::DuplicateTrackerRoots {
                first_instance_id,
                duplicate_instance_id: instance_id.to_string(),
                tracker_identity: work_unit_identity,
            });
        }
        validate_deployment_identity(instance_id, &tracker_identity, project)?;
    }

    Ok(())
}

fn validate_deployment_identity(
    instance_id: &str,
    handle_identity: &str,
    project: &ForgeProject,
) -> Result<(), ScopedWorkUnitError> {
    if project.trackers.is_empty() || project.tracker_by_identity(handle_identity).is_some() {
        return Ok(());
    }
    Err(ScopedWorkUnitError::DeploymentDisagreement {
        instance_id: instance_id.to_string(),
        handle_identity: handle_identity.to_string(),
        configured_identities: project
            .trackers
            .iter()
            .map(|tracker| tracker.identity.clone())
            .collect(),
    })
}

fn instance_work_unit_number(instance_id: &str) -> Option<u64> {
    let rest = instance_id.strip_prefix("work-unit-")?;
    let number = rest.split_once('-').map_or(rest, |(number, _)| number);
    number.parse().ok()
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;
    use crate::forge_address::{ForgeProject, RawForgeInstance, RawForges, RawRepository};
    use crate::{ArtifactStore, ArtifactType};

    fn work_unit_store(dir: &TempDir) -> ArtifactStore {
        ArtifactStore::new(
            vec![ArtifactType {
                name: "work-unit".to_string(),
                schema: json!({
                    "type": "object",
                    "required": ["title", "description", "acceptance_criteria"],
                    "properties": {
                        "title": { "type": "string" },
                        "description": { "type": "string" },
                        "acceptance_criteria": {
                            "type": "array",
                            "items": { "type": "string" }
                        },
                        "handle": { "type": "object" }
                    }
                }),
            }],
            dir.path().join("store"),
        )
        .unwrap()
    }

    fn github_project() -> ForgeProject {
        ForgeProject::resolve(RawForges {
            instances: vec![RawForgeInstance {
                id: "github-com".to_string(),
                forge_type: "github".to_string(),
                host: Some("github.com".to_string()),
                git_host: None,
                tracker_host: None,
            }],
            repositories: vec![RawRepository {
                id: "runa".to_string(),
                instance: "github-com".to_string(),
                owner: "tesserine".to_string(),
                name: "runa".to_string(),
            }],
            trackers: Vec::new(),
        })
        .unwrap()
    }

    fn github_work_unit(
        number: u64,
        tracker_identity: &str,
        work_unit_identity: &str,
    ) -> serde_json::Value {
        json!({
            "title": "Canonical scope",
            "description": "Validate canonical scoped identity",
            "acceptance_criteria": ["Reject mismatched handles"],
            "handle": {
                "forge_tag": "github",
                "tracker_identity": tracker_identity,
                "work_unit_identity": work_unit_identity,
                "number": number
            }
        })
    }

    #[test]
    fn exact_work_unit_id_accepts_matching_full_tracker_identity() {
        let tmp = TempDir::new().unwrap();
        let mut store = work_unit_store(&tmp);
        let artifact = github_work_unit(
            163,
            "github@github.com/tracker/tesserine/runa",
            "github@github.com/tracker/tesserine/runa#163",
        );
        let artifact_path = tmp.path().join("work-unit-163.json");
        std::fs::write(&artifact_path, artifact.to_string()).unwrap();
        store
            .record_with_timestamp("work-unit", "work-unit-163", &artifact_path, &artifact, 1)
            .unwrap();

        let result =
            validate_scoped_work_unit_with_project(&store, "work-unit-163", &github_project());

        assert_eq!(result, Ok(()));
    }

    #[test]
    fn handle_read_rejects_unsupported_forge_type_with_shared_error() {
        let tmp = TempDir::new().unwrap();
        let mut store = work_unit_store(&tmp);
        let artifact = json!({
            "title": "Bad forge",
            "description": "Reject bad forge",
            "acceptance_criteria": ["Unsupported forge types fail"],
            "handle": {
                "forge_tag": "gitlab",
                "tracker_identity": "gitlab@example/tracker/tesserine/runa",
                "work_unit_identity": "gitlab@example/tracker/tesserine/runa#9",
                "number": 9
            }
        });
        let artifact_path = tmp.path().join("work-unit-9.json");
        std::fs::write(&artifact_path, artifact.to_string()).unwrap();
        store
            .record_with_timestamp("work-unit", "work-unit-9", &artifact_path, &artifact, 1)
            .unwrap();

        let error =
            validate_scoped_work_unit_with_project(&store, "work-unit-9", &github_project())
                .unwrap_err();

        assert!(matches!(
            error,
            ScopedWorkUnitError::ForgeAddress(ForgeAddressError::UnsupportedForgeType { forge_type })
                if forge_type == "gitlab"
        ));
    }

    #[test]
    fn exact_work_unit_id_rejects_duplicate_tracker_roots() {
        let tmp = TempDir::new().unwrap();
        let mut store = work_unit_store(&tmp);
        let artifact = github_work_unit(
            163,
            "github@github.com/tracker/tesserine/runa",
            "github@github.com/tracker/tesserine/runa#163",
        );
        for instance_id in ["work-unit-163-a", "work-unit-163-b"] {
            let path = tmp.path().join(format!("{instance_id}.json"));
            std::fs::write(&path, artifact.to_string()).unwrap();
            store
                .record_with_timestamp("work-unit", instance_id, &path, &artifact, 1)
                .unwrap();
        }

        let error =
            validate_scoped_work_unit_with_project(&store, "work-unit-163-a", &github_project())
                .unwrap_err();

        assert!(matches!(
            error,
            ScopedWorkUnitError::DuplicateTrackerRoots { tracker_identity, .. }
                if tracker_identity == "github@github.com/tracker/tesserine/runa#163"
        ));
    }

    #[test]
    fn exact_work_unit_id_rejects_explicit_identity_that_disagrees_with_handle_number() {
        let tmp = TempDir::new().unwrap();
        let mut store = work_unit_store(&tmp);
        let artifact = github_work_unit(
            163,
            "github@github.com/tracker/tesserine/runa",
            "github@github.com/tracker/tesserine/runa#164",
        );
        let artifact_path = tmp.path().join("work-unit-163.json");
        std::fs::write(&artifact_path, artifact.to_string()).unwrap();
        store
            .record_with_timestamp("work-unit", "work-unit-163", &artifact_path, &artifact, 1)
            .unwrap();

        let error =
            validate_scoped_work_unit_with_project(&store, "work-unit-163", &github_project())
                .unwrap_err();

        assert!(matches!(
            error,
            ScopedWorkUnitError::ForgeAddress(ForgeAddressError::MalformedPayload(detail))
                if detail.contains("work_unit_identity")
                    && detail.contains("github@github.com/tracker/tesserine/runa#164")
                    && detail.contains("github@github.com/tracker/tesserine/runa#163")
        ));
    }
}
