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
//!   identity must agree with the active forge address resolved from the
//!   project payload (`github:<owner>/<name>` or `sourcehut:<tracker_id>`).
//! - Endpoint and host resolution are deliberately outside scoped identity
//!   validation; identity is textual agreement, not network reachability.
//!
//! [`docs/interface-contract.md`]: https://github.com/tesserine/runa/blob/main/docs/interface-contract.md

use std::collections::HashMap;
use std::fmt;

use serde_json::{Value, json};

use crate::project::ForgeConfig;
use crate::store::{ArtifactStore, ValidationStatus};

pub const RUNA_FORGE_TYPE: &str = "RUNA_FORGE_TYPE";
pub const RUNA_FORGE_OWNER: &str = "RUNA_FORGE_OWNER";
pub const RUNA_FORGE_NAME: &str = "RUNA_FORGE_NAME";
pub const RUNA_FORGE_TRACKER_ID: &str = "RUNA_FORGE_TRACKER_ID";
pub const RUNA_PROJECT_FORGE_ADDRESSES: &str = "RUNA_PROJECT_FORGE_ADDRESSES";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedForgeIdentity {
    pub forge_type: String,
    pub owner: Option<String>,
    pub name: Option<String>,
    pub tracker_id: Option<String>,
}

impl ResolvedForgeIdentity {
    fn environment(&self) -> HashMap<String, String> {
        let mut environment =
            HashMap::from([(RUNA_FORGE_TYPE.to_string(), self.forge_type.clone())]);
        insert_if_present(&mut environment, RUNA_FORGE_OWNER, self.owner.as_deref());
        insert_if_present(&mut environment, RUNA_FORGE_NAME, self.name.as_deref());
        insert_if_present(
            &mut environment,
            RUNA_FORGE_TRACKER_ID,
            self.tracker_id.as_deref(),
        );
        environment
    }

    fn active_deployment_identity(&self) -> Result<String, ScopedWorkUnitError> {
        match self.forge_type.as_str() {
            "github" => {
                let owner = self.required_atom(RUNA_FORGE_OWNER, self.owner.as_deref())?;
                let name = self.required_atom(RUNA_FORGE_NAME, self.name.as_deref())?;
                Ok(format!("github:{owner}/{name}"))
            }
            "sourcehut" => {
                let tracker_id =
                    self.required_atom(RUNA_FORGE_TRACKER_ID, self.tracker_id.as_deref())?;
                Ok(format!("sourcehut:{tracker_id}"))
            }
            other => Ok(other.to_string()),
        }
    }

    fn address_payload(&self) -> Option<String> {
        let instance = "default";
        match self.forge_type.as_str() {
            "github" => {
                let owner = self.owner.as_deref()?;
                let name = self.name.as_deref()?;
                Some(
                    json!({
                        "version": 1,
                        "instances": {
                            "default": {
                                "type": "github",
                                "host": "github.com",
                            }
                        },
                        "repositories": [{
                            "id": "default",
                            "instance": instance,
                            "owner": owner,
                            "name": name,
                        }],
                        "trackers": [],
                    })
                    .to_string(),
                )
            }
            "sourcehut" => {
                let tracker_id = self.tracker_id.as_deref()?;
                Some(
                    json!({
                        "version": 1,
                        "instances": {
                            "default": {
                                "type": "sourcehut",
                                "git_host": "git.sr.ht",
                                "tracker_host": "todo.sr.ht",
                            }
                        },
                        "repositories": [],
                        "trackers": [{
                            "id": "default",
                            "instance": instance,
                            "owner": self.owner.as_deref().unwrap_or(""),
                            "name": self.name.as_deref().unwrap_or(""),
                            "tracker_id": tracker_id,
                        }],
                    })
                    .to_string(),
                )
            }
            _ => None,
        }
    }

    fn required_atom<'a>(
        &self,
        variable: &'static str,
        value: Option<&'a str>,
    ) -> Result<&'a str, ScopedWorkUnitError> {
        value
            .filter(|value| !value.is_empty())
            .ok_or(ScopedWorkUnitError::MissingDeploymentIdentity { variable })
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
        variable: &'static str,
    },
    DeploymentDisagreement {
        instance_id: String,
        handle_identity: String,
        active_identity: String,
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
            ScopedWorkUnitError::MissingDeploymentIdentity { variable } => write!(
                f,
                "missing required deployment identity atom '{variable}' for scoped work-unit validation"
            ),
            ScopedWorkUnitError::DeploymentDisagreement {
                instance_id,
                handle_identity,
                active_identity,
            } => write!(
                f,
                "work-unit instance '{instance_id}' belongs to {handle_identity}, which disagrees with active deployment {active_identity}"
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
    let identity = resolve_forge_identity(&ForgeConfig::default());
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

pub fn resolve_forge_identity(config: &ForgeConfig) -> ResolvedForgeIdentity {
    if let Ok(payload) = std::env::var(RUNA_PROJECT_FORGE_ADDRESSES) {
        if let Some(identity) = resolve_forge_identity_from_payload(&payload) {
            return identity;
        }
    }

    ResolvedForgeIdentity {
        forge_type: resolve_atom(RUNA_FORGE_TYPE, config.forge_type.as_deref())
            .unwrap_or_else(|| "github".to_string()),
        owner: resolve_atom(RUNA_FORGE_OWNER, config.owner.as_deref()),
        name: resolve_atom(RUNA_FORGE_NAME, config.name.as_deref()),
        tracker_id: resolve_atom(RUNA_FORGE_TRACKER_ID, config.tracker_id.as_deref()),
    }
}

pub fn resolve_forge_address_payload(config: &ForgeConfig) -> Option<String> {
    std::env::var(RUNA_PROJECT_FORGE_ADDRESSES)
        .ok()
        .and_then(|value| normalize_atom(Some(value.as_str())))
        .or_else(|| resolve_forge_identity(config).address_payload())
}

pub fn resolve_forge_environment(config: &ForgeConfig) -> HashMap<String, String> {
    resolve_forge_identity(config).environment()
}

fn resolve_forge_identity_from_payload(payload: &str) -> Option<ResolvedForgeIdentity> {
    let payload = serde_json::from_str::<Value>(payload).ok()?;
    let instances = payload.get("instances")?.as_object()?;
    let repositories = payload
        .get("repositories")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let trackers = payload
        .get("trackers")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);

    match (repositories, trackers) {
        ([repository], []) => identity_from_resource(instances, repository, false),
        ([], [tracker]) => identity_from_resource(instances, tracker, true),
        _ => None,
    }
}

fn identity_from_resource(
    instances: &serde_json::Map<String, Value>,
    resource: &Value,
    tracker: bool,
) -> Option<ResolvedForgeIdentity> {
    let resource = resource.as_object()?;
    let instance_id = resource.get("instance")?.as_str()?;
    let instance = instances.get(instance_id)?.as_object()?;
    let forge_type = instance.get("type")?.as_str()?.to_string();
    let owner = resource
        .get("owner")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let name = resource
        .get("name")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let tracker_id = if tracker {
        resource.get("tracker_id").and_then(|value| {
            value
                .as_str()
                .map(str::to_string)
                .or_else(|| value.as_u64().map(|number| number.to_string()))
        })
    } else {
        None
    };

    Some(ResolvedForgeIdentity {
        forge_type,
        owner,
        name,
        tracker_id,
    })
}

fn resolve_atom(variable: &'static str, config_value: Option<&str>) -> Option<String> {
    std::env::var(variable)
        .ok()
        .and_then(|value| normalize_atom(Some(value.as_str())))
        .or_else(|| normalize_atom(config_value))
}

fn normalize_atom(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn insert_if_present(
    environment: &mut HashMap<String, String>,
    variable: &'static str,
    value: Option<&str>,
) {
    if let Some(value) = normalize_atom(value) {
        environment.insert(variable.to_string(), value);
    }
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
    let active_identity = identity.active_deployment_identity()?;
    if handle_forge != identity.forge_type || handle_identity != active_identity {
        return Err(ScopedWorkUnitError::DeploymentDisagreement {
            instance_id: instance_id.to_string(),
            handle_identity,
            active_identity,
        });
    }

    Ok(())
}

fn deployment_identity_for_handle(
    handle: &serde_json::Map<String, Value>,
) -> Result<String, ScopedWorkUnitError> {
    match handle
        .get("forge_tag")
        .and_then(Value::as_str)
        .unwrap_or("")
    {
        "github" => {
            let repository = github_repository(
                handle
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
            .unwrap_or_default();
            Ok(format!("github:{repository}"))
        }
        "sourcehut" => {
            let tracker_id = handle
                .get("tracker_id")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            Ok(format!("sourcehut:{tracker_id}"))
        }
        forge => Ok(forge.to_string()),
    }
}

fn tracker_identity(handle: &serde_json::Map<String, Value>) -> Option<String> {
    match handle.get("forge_tag").and_then(Value::as_str)? {
        "github" => {
            let repository = github_repository(handle.get("url")?.as_str()?)?;
            let number = handle.get("number")?.as_u64()?;
            Some(format!("github:{repository}:{number}"))
        }
        "sourcehut" => {
            let tracker_id = handle.get("tracker_id")?.as_u64()?;
            let number = handle.get("number")?.as_u64()?;
            Some(format!("sourcehut:{tracker_id}:{number}"))
        }
        _ => None,
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
    use crate::test_helpers::EnvGuard;
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;
    use crate::project::ForgeConfig;
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
            forge_type: "github".to_string(),
            owner: Some(owner.to_string()),
            name: Some(name.to_string()),
            tracker_id: None,
        }
    }

    fn sourcehut_identity(tracker_id: u64) -> ResolvedForgeIdentity {
        ResolvedForgeIdentity {
            forge_type: "sourcehut".to_string(),
            owner: None,
            name: None,
            tracker_id: Some(tracker_id.to_string()),
        }
    }

    #[test]
    fn forge_environment_resolves_config_values_when_environment_is_unset() {
        let _env = EnvGuard::unset(&[
            "RUNA_FORGE_TYPE",
            "RUNA_FORGE_OWNER",
            "RUNA_FORGE_NAME",
            "RUNA_FORGE_TRACKER_ID",
        ]);
        let config = ForgeConfig {
            forge_type: Some("sourcehut".to_string()),
            owner: Some("operator".to_string()),
            name: Some("weforge".to_string()),
            tracker_id: Some("4".to_string()),
        };

        let environment = resolve_forge_environment(&config);

        assert_eq!(
            environment.get("RUNA_FORGE_TYPE").map(String::as_str),
            Some("sourcehut")
        );
        assert_eq!(
            environment.get("RUNA_FORGE_OWNER").map(String::as_str),
            Some("operator")
        );
        assert_eq!(
            environment.get("RUNA_FORGE_NAME").map(String::as_str),
            Some("weforge")
        );
        assert_eq!(
            environment.get("RUNA_FORGE_TRACKER_ID").map(String::as_str),
            Some("4")
        );
    }

    #[test]
    fn forge_environment_resolves_environment_values_over_config_values() {
        let _env = EnvGuard::set(&[
            ("RUNA_FORGE_TYPE", "github"),
            ("RUNA_FORGE_OWNER", "env-owner"),
            ("RUNA_FORGE_NAME", "env-name"),
            ("RUNA_FORGE_TRACKER_ID", "9"),
        ]);
        let config = ForgeConfig {
            forge_type: Some("sourcehut".to_string()),
            owner: Some("config-owner".to_string()),
            name: Some("config-name".to_string()),
            tracker_id: Some("4".to_string()),
        };

        let environment = resolve_forge_environment(&config);

        assert_eq!(
            environment.get("RUNA_FORGE_TYPE").map(String::as_str),
            Some("github")
        );
        assert_eq!(
            environment.get("RUNA_FORGE_OWNER").map(String::as_str),
            Some("env-owner")
        );
        assert_eq!(
            environment.get("RUNA_FORGE_NAME").map(String::as_str),
            Some("env-name")
        );
        assert_eq!(
            environment.get("RUNA_FORGE_TRACKER_ID").map(String::as_str),
            Some("9")
        );
    }

    #[test]
    fn forge_environment_materializes_default_github_type_when_config_and_environment_omit_type() {
        let _env = EnvGuard::unset(&[
            "RUNA_FORGE_TYPE",
            "RUNA_FORGE_OWNER",
            "RUNA_FORGE_NAME",
            "RUNA_FORGE_TRACKER_ID",
        ]);
        let config = ForgeConfig {
            forge_type: None,
            owner: Some("tesserine".to_string()),
            name: Some("runa".to_string()),
            tracker_id: None,
        };

        let environment = resolve_forge_environment(&config);

        assert_eq!(
            environment.get("RUNA_FORGE_TYPE").map(String::as_str),
            Some("github")
        );
        assert_eq!(
            environment.get("RUNA_FORGE_OWNER").map(String::as_str),
            Some("tesserine")
        );
        assert_eq!(
            environment.get("RUNA_FORGE_NAME").map(String::as_str),
            Some("runa")
        );
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
        let _env = EnvGuard::unset(&[
            "RUNA_FORGE_TYPE",
            "RUNA_FORGE_OWNER",
            "RUNA_FORGE_NAME",
            "RUNA_FORGE_TRACKER_ID",
        ]);
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
        let identity = resolve_forge_identity(&ForgeConfig {
            forge_type: Some("sourcehut".to_string()),
            owner: Some("operator".to_string()),
            name: Some("weforge".to_string()),
            tracker_id: Some("4".to_string()),
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
