//! Canonical scoped work-unit identity validation, shared by the CLI
//! commands and `runa-mcp`.
//!
//! Governing contract: [`docs/interface-contract.md`] — runa owns scope
//! *identity*; the methodology owns the `work-unit` schema and the
//! semantics of its content. This module holds the runtime checks that
//! JSON Schema cannot express.
//!
//! Invariants:
//!
//! - **The canonical scope set is the recorded `work-unit` instance ids —
//!   including invalid and malformed records.** A broken record still
//!   occupies its identity; excluding it would let a session silently open
//!   against a scope that exists but failed validation.
//! - For valid tracker-backed roots: the canonical instance id's tracker
//!   number must agree with the handle number; duplicate tracker
//!   identities across roots are rejected; and the handle's deployment
//!   identity must agree with the active deployment resolved from the
//!   configured target project (`github:<owner>/<name>` or
//!   `sourcehut:<tracker_id>`).
//! - Endpoint and host resolution are deliberately outside scoped identity
//!   validation; identity is textual agreement, not network reachability.
//!
//! [`docs/interface-contract.md`]: https://github.com/tesserine/runa/blob/main/docs/interface-contract.md

use std::collections::HashMap;
use std::fmt;

use serde_json::Value;

use crate::project::{ForgeType, RepositoryConfig, TargetProjectConfig, TrackerConfig};
use crate::store::{ArtifactStore, ValidationStatus};

pub const RETIRED_FORGE_ENV: [&str; 4] = [
    "RUNA_FORGE_TYPE",
    "RUNA_FORGE_OWNER",
    "RUNA_FORGE_NAME",
    "RUNA_FORGE_TRACKER_ID",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedForgeIdentity {
    pub forge_type: ForgeType,
    pub repositories: Vec<RepositoryConfig>,
    pub trackers: Vec<TrackerConfig>,
}

impl ResolvedForgeIdentity {
    fn environment(&self) -> HashMap<String, String> {
        crate::project::target_project_env(&TargetProjectConfig {
            forge_type: self.forge_type,
            repositories: self.repositories.clone(),
            trackers: self.trackers.clone(),
        })
        .unwrap_or_default()
        .into_iter()
        .collect()
    }

    pub fn active_deployment_identity(&self) -> Result<String, ScopedWorkUnitError> {
        let identities = self.configured_tracker_identities();
        match identities.as_slice() {
            [identity] => Ok(identity.clone()),
            [] => Err(ScopedWorkUnitError::MissingDeploymentIdentity {
                detail: "target project declares no tracker identity".to_string(),
            }),
            _ => Err(ScopedWorkUnitError::AmbiguousTrackerIdentity),
        }
    }

    pub fn configured_tracker_identities(&self) -> Vec<String> {
        match self.forge_type {
            ForgeType::Github => {
                if self.trackers.is_empty() {
                    self.repositories
                        .iter()
                        .map(|repo| format!("github:{}/{}", repo.owner, repo.name))
                        .collect()
                } else {
                    self.trackers
                        .iter()
                        .filter_map(|tracker| {
                            let selector = tracker.repository.as_deref()?;
                            let repo = self
                                .repositories
                                .iter()
                                .find(|repo| repo.selector == selector)?;
                            Some(format!("github:{}/{}", repo.owner, repo.name))
                        })
                        .collect()
                }
            }
            ForgeType::Sourcehut => self
                .trackers
                .iter()
                .filter_map(|tracker| {
                    tracker
                        .tracker_id
                        .as_deref()
                        .map(|tracker_id| format!("sourcehut:{tracker_id}"))
                })
                .collect(),
        }
    }

    pub fn repository_by_selector(&self, selector: &str) -> Option<&RepositoryConfig> {
        self.repositories
            .iter()
            .find(|repository| repository.selector == selector)
    }

    pub fn tracker_by_selector(&self, selector: &str) -> Option<&TrackerConfig> {
        self.trackers
            .iter()
            .find(|tracker| tracker.selector == selector)
    }
}

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
    MissingDeploymentIdentity {
        detail: String,
    },
    AmbiguousTrackerIdentity,
    DeploymentDisagreement {
        instance_id: String,
        handle_identity: String,
        configured_identities: Vec<String>,
    },
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
            ScopedWorkUnitError::MissingDeploymentIdentity { detail } => write!(
                f,
                "missing configured target-project identity for scoped work-unit validation: {detail}"
            ),
            ScopedWorkUnitError::AmbiguousTrackerIdentity => write!(
                f,
                "bare tracker reference is ambiguous because the target project declares multiple trackers"
            ),
            ScopedWorkUnitError::DeploymentDisagreement {
                instance_id,
                handle_identity,
                configured_identities,
            } => write!(
                f,
                "work-unit instance '{instance_id}' belongs to {handle_identity}, which is not declared by the configured target project ({})",
                configured_identities.join(", ")
            ),
            ScopedWorkUnitError::WorkUnitScanIncomplete => write!(
                f,
                "the 'work-unit' artifact type was only partially scanned; the ticket cannot be resolved without complete work-unit scan trust"
            ),
        }
    }
}

impl std::error::Error for ScopedWorkUnitError {}

pub fn validate_scoped_work_unit(
    store: &ArtifactStore,
    supplied: &str,
) -> Result<(), ScopedWorkUnitError> {
    let identity = resolve_forge_identity(&TargetProjectConfig::default());
    validate_scoped_work_unit_with_identity(store, supplied, &identity)
}

/// Enforce tracker-handle consistency across all recorded `work-unit` instances
/// without requiring a supplied canonical id.
///
/// Used during promised-scope entry, where the session has a forge ticket
/// reference rather than a recorded instance id. Runs the same content checks as
/// [`validate_scoped_work_unit_with_identity`] (tracker-number agreement,
/// duplicate-root rejection, deployment-identity agreement) over the store.
pub fn validate_tracker_consistency(
    store: &ArtifactStore,
    identity: &ResolvedForgeIdentity,
) -> Result<(), ScopedWorkUnitError> {
    validate_tracker_content(store, identity)
}

/// Find the valid `work-unit` instance whose tracker handle identity equals
/// `target` (e.g. `github:owner/name:14`), if any.
///
/// Reads handles from the local store only; performs no forge access. Returns
/// the first match in store iteration order; callers run
/// [`validate_tracker_consistency`] first to reject duplicate tracker roots, so
/// at most one instance can match a given identity in a consistent store.
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
        if tracker_identity(handle).as_deref() == Some(target) {
            return Some(instance_id.to_string());
        }
    }
    None
}

pub fn resolve_forge_identity(config: &TargetProjectConfig) -> ResolvedForgeIdentity {
    ResolvedForgeIdentity {
        forge_type: config.forge_type,
        repositories: config.repositories.clone(),
        trackers: config.trackers.clone(),
    }
}

pub fn resolve_forge_environment(config: &TargetProjectConfig) -> HashMap<String, String> {
    resolve_forge_identity(config).environment()
}

pub fn validate_scoped_work_unit_with_identity(
    store: &ArtifactStore,
    supplied: &str,
    identity: &ResolvedForgeIdentity,
) -> Result<(), ScopedWorkUnitError> {
    let available: Vec<String> = store
        .instances_of("work-unit", None)
        .into_iter()
        .map(|(instance_id, _)| instance_id.to_string())
        .collect();

    if available.is_empty() || available.iter().any(|id| id == supplied) {
        return validate_tracker_content(store, identity);
    }

    Err(ScopedWorkUnitError::NonCanonical {
        supplied: supplied.to_string(),
        available,
    })
}

fn validate_tracker_content(
    store: &ArtifactStore,
    identity: &ResolvedForgeIdentity,
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

        let Some(tracker_identity) = tracker_identity(handle) else {
            continue;
        };
        if let Some(first_instance_id) =
            tracker_roots.insert(tracker_identity.clone(), instance_id.to_string())
        {
            return Err(ScopedWorkUnitError::DuplicateTrackerRoots {
                first_instance_id,
                duplicate_instance_id: instance_id.to_string(),
                tracker_identity,
            });
        }
        validate_deployment_identity(instance_id, handle, identity)?;
    }

    Ok(())
}

fn validate_deployment_identity(
    instance_id: &str,
    handle: &serde_json::Map<String, Value>,
    identity: &ResolvedForgeIdentity,
) -> Result<(), ScopedWorkUnitError> {
    let handle_forge = handle
        .get("forge_tag")
        .and_then(Value::as_str)
        .unwrap_or("");
    let handle_identity = deployment_identity_for_handle(handle)?;
    let configured = identity.configured_tracker_identities();
    if handle_forge != identity.forge_type.as_str()
        || !configured.iter().any(|item| item == &handle_identity)
    {
        return Err(ScopedWorkUnitError::DeploymentDisagreement {
            instance_id: instance_id.to_string(),
            handle_identity,
            configured_identities: configured,
        });
    }

    Ok(())
}

fn deployment_identity_for_handle(
    handle: &serde_json::Map<String, Value>,
) -> Result<String, ScopedWorkUnitError> {
    match ForgeType::parse(
        handle
            .get("forge_tag")
            .and_then(Value::as_str)
            .unwrap_or(""),
    ) {
        Ok(ForgeType::Github) => {
            let repository = github_repository(
                handle
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
            .unwrap_or_default();
            Ok(format!("github:{repository}"))
        }
        Ok(ForgeType::Sourcehut) => {
            let tracker_id = handle
                .get("tracker_id")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            Ok(format!("sourcehut:{tracker_id}"))
        }
        Err(error) => Ok(error.forge_type),
    }
}

fn tracker_identity(handle: &serde_json::Map<String, Value>) -> Option<String> {
    match ForgeType::parse(handle.get("forge_tag").and_then(Value::as_str)?).ok()? {
        ForgeType::Github => {
            let repository = github_repository(handle.get("url")?.as_str()?)?;
            let number = handle.get("number")?.as_u64()?;
            Some(format!("github:{repository}:{number}"))
        }
        ForgeType::Sourcehut => {
            let tracker_id = handle.get("tracker_id")?.as_u64()?;
            let number = handle.get("number")?.as_u64()?;
            Some(format!("sourcehut:{tracker_id}:{number}"))
        }
    }
}

fn github_repository(url: &str) -> Option<&str> {
    let path = url.strip_prefix("https://github.com/")?;
    let (repository, _) = path.split_once("/issues/")?;
    Some(repository)
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
    use crate::project::{ForgeType, RepositoryConfig, TargetProjectConfig, TrackerConfig};
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

    fn github_work_unit(number: u64) -> serde_json::Value {
        json!({
            "title": "Canonical scope",
            "description": "Validate canonical scoped identity",
            "acceptance_criteria": ["Reject mismatched handles"],
            "handle": {
                "forge_tag": "github",
                "url": format!("https://github.com/tesserine/runa/issues/{number}"),
                "number": number
            }
        })
    }

    fn sourcehut_work_unit(tracker_id: u64, number: u64) -> serde_json::Value {
        json!({
            "title": "Canonical scope",
            "description": "Validate canonical scoped identity",
            "acceptance_criteria": ["Reject mismatched handles"],
            "handle": {
                "forge_tag": "sourcehut",
                "tracker_id": tracker_id,
                "number": number
            }
        })
    }

    fn github_identity(repository: &str) -> ResolvedForgeIdentity {
        let (owner, name) = repository.split_once('/').unwrap();
        ResolvedForgeIdentity {
            forge_type: ForgeType::Github,
            repositories: vec![RepositoryConfig {
                selector: "default".to_string(),
                host: "github.com".to_string(),
                owner: owner.to_string(),
                name: name.to_string(),
            }],
            trackers: vec![TrackerConfig {
                selector: "default".to_string(),
                repository: Some("default".to_string()),
                host: None,
                owner: None,
                name: None,
                tracker_id: None,
            }],
        }
    }

    fn sourcehut_identity(tracker_id: u64) -> ResolvedForgeIdentity {
        ResolvedForgeIdentity {
            forge_type: ForgeType::Sourcehut,
            repositories: Vec::new(),
            trackers: vec![TrackerConfig {
                selector: "default".to_string(),
                repository: None,
                host: Some("weforge.build".to_string()),
                owner: Some("operator".to_string()),
                name: Some("weforge".to_string()),
                tracker_id: Some(tracker_id.to_string()),
            }],
        }
    }

    #[test]
    fn forge_environment_exports_structured_target_project_payload() {
        let config = TargetProjectConfig {
            forge_type: ForgeType::Sourcehut,
            repositories: vec![RepositoryConfig {
                selector: "groundwork".to_string(),
                host: "weforge.build".to_string(),
                owner: "operator".to_string(),
                name: "weforge".to_string(),
            }],
            trackers: vec![TrackerConfig {
                selector: "todo".to_string(),
                repository: None,
                host: Some("weforge.build".to_string()),
                owner: Some("operator".to_string()),
                name: Some("weforge".to_string()),
                tracker_id: Some("4".to_string()),
            }],
        };

        let environment = resolve_forge_environment(&config);
        let payload: serde_json::Value = serde_json::from_str(
            environment
                .get(crate::project::RUNA_TARGET_PROJECT)
                .unwrap(),
        )
        .unwrap();

        assert_eq!(payload["forge_type"], "sourcehut");
        assert_eq!(payload["repositories"][0]["selector"], "groundwork");
        assert_eq!(payload["trackers"][0]["tracker_id"], "4");
    }

    #[test]
    fn forge_environment_does_not_emit_retired_forge_atoms() {
        let config = TargetProjectConfig {
            forge_type: ForgeType::Github,
            repositories: vec![RepositoryConfig {
                selector: "runa".to_string(),
                host: "github.com".to_string(),
                owner: "tesserine".to_string(),
                name: "runa".to_string(),
            }],
            trackers: Vec::new(),
        };

        let environment = resolve_forge_environment(&config);

        for retired in RETIRED_FORGE_ENV {
            assert!(
                !environment.contains_key(retired),
                "{retired} should be retired"
            );
        }
    }

    #[test]
    fn exact_work_unit_id_without_slug_accepts_matching_github_deployment() {
        let tmp = TempDir::new().unwrap();
        let mut store = work_unit_store(&tmp);
        let artifact = github_work_unit(163);
        let artifact_path = tmp.path().join("work-unit-163.json");
        std::fs::write(&artifact_path, artifact.to_string()).unwrap();
        store
            .record_with_timestamp("work-unit", "work-unit-163", &artifact_path, &artifact, 1)
            .unwrap();

        let result = validate_scoped_work_unit_with_identity(
            &store,
            "work-unit-163",
            &github_identity("tesserine/runa"),
        );

        assert_eq!(result, Ok(()));
    }

    #[test]
    fn exact_work_unit_id_rejects_tracker_number_disagreement() {
        let tmp = TempDir::new().unwrap();
        let mut store = work_unit_store(&tmp);
        let artifact_path = tmp.path().join("work-unit-163-scope.json");
        std::fs::write(&artifact_path, github_work_unit(164).to_string()).unwrap();
        store
            .record_with_timestamp(
                "work-unit",
                "work-unit-163-scope",
                &artifact_path,
                &github_work_unit(164),
                1,
            )
            .unwrap();

        let result = validate_scoped_work_unit(&store, "work-unit-163-scope");

        assert_eq!(
            result,
            Err(ScopedWorkUnitError::TrackerNumberDisagreement {
                instance_id: "work-unit-163-scope".to_string(),
                handle_number: 164,
            })
        );
    }

    #[test]
    fn exact_work_unit_id_rejects_unparseable_tracker_number_without_disagreement() {
        let tmp = TempDir::new().unwrap();
        let mut store = work_unit_store(&tmp);
        let artifact = github_work_unit(163);
        let artifact_path = tmp.path().join("work-unit-scope.json");
        std::fs::write(&artifact_path, artifact.to_string()).unwrap();
        store
            .record_with_timestamp("work-unit", "work-unit-scope", &artifact_path, &artifact, 1)
            .unwrap();

        let result = validate_scoped_work_unit(&store, "work-unit-scope");

        assert_eq!(
            result,
            Err(ScopedWorkUnitError::TrackerNumberParseFailure {
                instance_id: "work-unit-scope".to_string(),
            })
        );
    }

    #[test]
    fn exact_work_unit_id_rejects_duplicate_github_tracker_roots() {
        let tmp = TempDir::new().unwrap();
        let mut store = work_unit_store(&tmp);
        for instance_id in ["work-unit-163-scope-a", "work-unit-163-scope-b"] {
            let artifact_path = tmp.path().join(format!("{instance_id}.json"));
            let artifact = github_work_unit(163);
            std::fs::write(&artifact_path, artifact.to_string()).unwrap();
            store
                .record_with_timestamp("work-unit", instance_id, &artifact_path, &artifact, 1)
                .unwrap();
        }

        let result = validate_scoped_work_unit(&store, "work-unit-163-scope-a");

        assert!(result.is_err());
    }

    #[test]
    fn exact_work_unit_id_rejects_github_handle_from_foreign_deployment() {
        let tmp = TempDir::new().unwrap();
        let mut store = work_unit_store(&tmp);
        let artifact = github_work_unit(163);
        let artifact_path = tmp.path().join("work-unit-163-scope.json");
        std::fs::write(&artifact_path, artifact.to_string()).unwrap();
        store
            .record_with_timestamp(
                "work-unit",
                "work-unit-163-scope",
                &artifact_path,
                &artifact,
                1,
            )
            .unwrap();
        let identity = github_identity("tesserine/groundwork");

        let result =
            validate_scoped_work_unit_with_identity(&store, "work-unit-163-scope", &identity);

        assert!(result.is_err());
    }

    #[test]
    fn exact_work_unit_id_accepts_matching_github_deployment() {
        let tmp = TempDir::new().unwrap();
        let mut store = work_unit_store(&tmp);
        let artifact = github_work_unit(163);
        let artifact_path = tmp.path().join("work-unit-163-scope.json");
        std::fs::write(&artifact_path, artifact.to_string()).unwrap();
        store
            .record_with_timestamp(
                "work-unit",
                "work-unit-163-scope",
                &artifact_path,
                &artifact,
                1,
            )
            .unwrap();

        let result = validate_scoped_work_unit_with_identity(
            &store,
            "work-unit-163-scope",
            &github_identity("tesserine/runa"),
        );

        assert_eq!(result, Ok(()));
    }

    #[test]
    fn exact_work_unit_id_rejects_duplicate_sourcehut_tracker_roots() {
        let tmp = TempDir::new().unwrap();
        let mut store = work_unit_store(&tmp);
        for instance_id in ["work-unit-163-scope-a", "work-unit-163-scope-b"] {
            let artifact_path = tmp.path().join(format!("{instance_id}.json"));
            let artifact = sourcehut_work_unit(4, 163);
            std::fs::write(&artifact_path, artifact.to_string()).unwrap();
            store
                .record_with_timestamp("work-unit", instance_id, &artifact_path, &artifact, 1)
                .unwrap();
        }

        let result = validate_scoped_work_unit_with_identity(
            &store,
            "work-unit-163-scope-a",
            &sourcehut_identity(4),
        );

        assert!(result.is_err());
    }

    #[test]
    fn exact_work_unit_id_rejects_sourcehut_handle_from_foreign_deployment() {
        let tmp = TempDir::new().unwrap();
        let mut store = work_unit_store(&tmp);
        let artifact = sourcehut_work_unit(4, 163);
        let artifact_path = tmp.path().join("work-unit-163-scope.json");
        std::fs::write(&artifact_path, artifact.to_string()).unwrap();
        store
            .record_with_timestamp(
                "work-unit",
                "work-unit-163-scope",
                &artifact_path,
                &artifact,
                1,
            )
            .unwrap();

        let result = validate_scoped_work_unit_with_identity(
            &store,
            "work-unit-163-scope",
            &sourcehut_identity(5),
        );

        assert!(result.is_err());
    }

    #[test]
    fn exact_work_unit_id_accepts_matching_sourcehut_deployment_from_config() {
        let tmp = TempDir::new().unwrap();
        let mut store = work_unit_store(&tmp);
        let artifact = sourcehut_work_unit(4, 163);
        let artifact_path = tmp.path().join("work-unit-163-scope.json");
        std::fs::write(&artifact_path, artifact.to_string()).unwrap();
        store
            .record_with_timestamp(
                "work-unit",
                "work-unit-163-scope",
                &artifact_path,
                &artifact,
                1,
            )
            .unwrap();
        let identity = resolve_forge_identity(&TargetProjectConfig {
            forge_type: ForgeType::Sourcehut,
            repositories: Vec::new(),
            trackers: vec![TrackerConfig {
                selector: "todo".to_string(),
                repository: None,
                host: Some("weforge.build".to_string()),
                owner: Some("operator".to_string()),
                name: Some("weforge".to_string()),
                tracker_id: Some("4".to_string()),
            }],
        });

        let result =
            validate_scoped_work_unit_with_identity(&store, "work-unit-163-scope", &identity);

        assert_eq!(result, Ok(()));
    }

    #[test]
    fn exact_work_unit_id_accepts_malformed_recorded_root_without_tracker_checks() {
        let tmp = TempDir::new().unwrap();
        let mut store = work_unit_store(&tmp);
        store
            .record_malformed(
                "work-unit",
                "work-unit-163-scope",
                &tmp.path().join("work-unit-163-scope.json"),
                br#"{ not json }"#,
                "expected value",
            )
            .unwrap();

        let result = validate_scoped_work_unit(&store, "work-unit-163-scope");

        assert_eq!(result, Ok(()));
    }
}
