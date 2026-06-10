use std::collections::HashMap;
use std::fmt;

use serde_json::Value;

use crate::project::ForgeConfig;
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
    MissingDeploymentIdentity {
        variable: &'static str,
    },
    DeploymentDisagreement {
        instance_id: String,
        handle_identity: String,
        active_identity: String,
    },
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
        }
    }
}

impl std::error::Error for ScopedWorkUnitError {}

pub fn validate_scoped_work_unit(
    store: &ArtifactStore,
    supplied: &str,
) -> Result<(), ScopedWorkUnitError> {
    let environment = resolve_forge_environment(&ForgeConfig::default());
    validate_scoped_work_unit_with_env(store, supplied, &environment)
}

pub fn resolve_forge_environment(config: &ForgeConfig) -> HashMap<String, String> {
    let mut environment: HashMap<String, String> = std::env::vars()
        .filter(|(name, value)| name.starts_with("GROUNDWORK_") && !value.is_empty())
        .collect();

    insert_config_env(
        &mut environment,
        "GROUNDWORK_FORGE_TYPE",
        config.forge_type.as_deref(),
    );
    environment
        .entry("GROUNDWORK_FORGE_TYPE".to_string())
        .or_insert_with(|| "github".to_string());
    insert_config_env(
        &mut environment,
        "GROUNDWORK_FORGE_OWNER",
        config.owner.as_deref(),
    );
    insert_config_env(
        &mut environment,
        "GROUNDWORK_FORGE_NAME",
        config.name.as_deref(),
    );
    insert_config_env(
        &mut environment,
        "GROUNDWORK_FORGE_TRACKER_ID",
        config.tracker_id.as_deref(),
    );

    environment
}

fn insert_config_env(
    environment: &mut HashMap<String, String>,
    variable: &'static str,
    value: Option<&str>,
) {
    if environment
        .get(variable)
        .is_some_and(|existing| !existing.is_empty())
    {
        return;
    }
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        environment.insert(variable.to_string(), value.to_string());
    }
}

pub fn validate_scoped_work_unit_with_env(
    store: &ArtifactStore,
    supplied: &str,
    environment: &HashMap<String, String>,
) -> Result<(), ScopedWorkUnitError> {
    let available: Vec<String> = store
        .instances_of("work-unit", None)
        .into_iter()
        .map(|(instance_id, _)| instance_id.to_string())
        .collect();

    if available.is_empty() || available.iter().any(|id| id == supplied) {
        return validate_tracker_content(store, environment);
    }

    Err(ScopedWorkUnitError::NonCanonical {
        supplied: supplied.to_string(),
        available,
    })
}

fn validate_tracker_content(
    store: &ArtifactStore,
    environment: &HashMap<String, String>,
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
        validate_deployment_identity(instance_id, handle, environment)?;
    }

    Ok(())
}

fn validate_deployment_identity(
    instance_id: &str,
    handle: &serde_json::Map<String, Value>,
    environment: &HashMap<String, String>,
) -> Result<(), ScopedWorkUnitError> {
    let active_forge = environment
        .get("GROUNDWORK_FORGE_TYPE")
        .filter(|value| !value.is_empty())
        .map(String::as_str)
        .unwrap_or("github");
    let handle_forge = handle
        .get("forge_tag")
        .and_then(Value::as_str)
        .unwrap_or("");

    let handle_identity = deployment_identity_for_handle(handle)?;
    let active_identity = active_deployment_identity(active_forge, environment)?;
    if handle_forge != active_forge || handle_identity != active_identity {
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

fn active_deployment_identity(
    active_forge: &str,
    environment: &HashMap<String, String>,
) -> Result<String, ScopedWorkUnitError> {
    match active_forge {
        "github" => {
            let owner = required_env(environment, "GROUNDWORK_FORGE_OWNER")?;
            let name = required_env(environment, "GROUNDWORK_FORGE_NAME")?;
            Ok(format!("github:{owner}/{name}"))
        }
        "sourcehut" => {
            let tracker_id = required_env(environment, "GROUNDWORK_FORGE_TRACKER_ID")?;
            Ok(format!("sourcehut:{tracker_id}"))
        }
        other => Ok(other.to_string()),
    }
}

fn required_env<'a>(
    environment: &'a HashMap<String, String>,
    variable: &'static str,
) -> Result<&'a str, ScopedWorkUnitError> {
    environment
        .get(variable)
        .filter(|value| !value.is_empty())
        .map(String::as_str)
        .ok_or(ScopedWorkUnitError::MissingDeploymentIdentity { variable })
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
    if instance_id.chars().all(|ch| ch.is_ascii_digit()) {
        return instance_id.parse().ok();
    }
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

    fn github_environment(repository: &str) -> HashMap<String, String> {
        let (owner, name) = repository.split_once('/').unwrap();
        HashMap::from([
            ("GROUNDWORK_FORGE_OWNER".to_string(), owner.to_string()),
            ("GROUNDWORK_FORGE_NAME".to_string(), name.to_string()),
        ])
    }

    fn sourcehut_environment(tracker_id: u64) -> HashMap<String, String> {
        HashMap::from([
            ("GROUNDWORK_FORGE_TYPE".to_string(), "sourcehut".to_string()),
            (
                "GROUNDWORK_FORGE_TRACKER_ID".to_string(),
                tracker_id.to_string(),
            ),
        ])
    }

    #[test]
    fn forge_environment_resolves_config_values_when_environment_is_unset() {
        let _env = EnvGuard::unset(&[
            "GROUNDWORK_FORGE_TYPE",
            "GROUNDWORK_FORGE_OWNER",
            "GROUNDWORK_FORGE_NAME",
            "GROUNDWORK_FORGE_TRACKER_ID",
        ]);
        let config = ForgeConfig {
            forge_type: Some("sourcehut".to_string()),
            owner: Some("operator".to_string()),
            name: Some("weforge".to_string()),
            tracker_id: Some("4".to_string()),
        };

        let environment = resolve_forge_environment(&config);

        assert_eq!(
            environment.get("GROUNDWORK_FORGE_TYPE").map(String::as_str),
            Some("sourcehut")
        );
        assert_eq!(
            environment
                .get("GROUNDWORK_FORGE_OWNER")
                .map(String::as_str),
            Some("operator")
        );
        assert_eq!(
            environment.get("GROUNDWORK_FORGE_NAME").map(String::as_str),
            Some("weforge")
        );
        assert_eq!(
            environment
                .get("GROUNDWORK_FORGE_TRACKER_ID")
                .map(String::as_str),
            Some("4")
        );
    }

    #[test]
    fn forge_environment_resolves_environment_values_over_config_values() {
        let _env = EnvGuard::set(&[
            ("GROUNDWORK_FORGE_TYPE", "github"),
            ("GROUNDWORK_FORGE_OWNER", "env-owner"),
            ("GROUNDWORK_FORGE_NAME", "env-name"),
            ("GROUNDWORK_FORGE_TRACKER_ID", "9"),
        ]);
        let config = ForgeConfig {
            forge_type: Some("sourcehut".to_string()),
            owner: Some("config-owner".to_string()),
            name: Some("config-name".to_string()),
            tracker_id: Some("4".to_string()),
        };

        let environment = resolve_forge_environment(&config);

        assert_eq!(
            environment.get("GROUNDWORK_FORGE_TYPE").map(String::as_str),
            Some("github")
        );
        assert_eq!(
            environment
                .get("GROUNDWORK_FORGE_OWNER")
                .map(String::as_str),
            Some("env-owner")
        );
        assert_eq!(
            environment.get("GROUNDWORK_FORGE_NAME").map(String::as_str),
            Some("env-name")
        );
        assert_eq!(
            environment
                .get("GROUNDWORK_FORGE_TRACKER_ID")
                .map(String::as_str),
            Some("9")
        );
    }

    #[test]
    fn forge_environment_materializes_default_github_type_when_config_and_environment_omit_type() {
        let _env = EnvGuard::unset(&[
            "GROUNDWORK_FORGE_TYPE",
            "GROUNDWORK_FORGE_OWNER",
            "GROUNDWORK_FORGE_NAME",
            "GROUNDWORK_FORGE_TRACKER_ID",
        ]);
        let config = ForgeConfig {
            forge_type: None,
            owner: Some("tesserine".to_string()),
            name: Some("runa".to_string()),
            tracker_id: None,
        };

        let environment = resolve_forge_environment(&config);

        assert_eq!(
            environment.get("GROUNDWORK_FORGE_TYPE").map(String::as_str),
            Some("github")
        );
        assert_eq!(
            environment
                .get("GROUNDWORK_FORGE_OWNER")
                .map(String::as_str),
            Some("tesserine")
        );
        assert_eq!(
            environment.get("GROUNDWORK_FORGE_NAME").map(String::as_str),
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

        let result = validate_scoped_work_unit_with_env(
            &store,
            "work-unit-163",
            &github_environment("tesserine/runa"),
        );

        assert_eq!(result, Ok(()));
    }

    #[test]
    fn bare_numeric_work_unit_id_accepts_matching_github_deployment() {
        let tmp = TempDir::new().unwrap();
        let mut store = work_unit_store(&tmp);
        let artifact = github_work_unit(163);
        let artifact_path = tmp.path().join("163.json");
        std::fs::write(&artifact_path, artifact.to_string()).unwrap();
        store
            .record_with_timestamp("work-unit", "163", &artifact_path, &artifact, 1)
            .unwrap();

        let result = validate_scoped_work_unit_with_env(
            &store,
            "163",
            &github_environment("tesserine/runa"),
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
        let environment = HashMap::from([
            (
                "GROUNDWORK_FORGE_OWNER".to_string(),
                "tesserine".to_string(),
            ),
            (
                "GROUNDWORK_FORGE_NAME".to_string(),
                "groundwork".to_string(),
            ),
        ]);

        let result =
            validate_scoped_work_unit_with_env(&store, "work-unit-163-scope", &environment);

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

        let result = validate_scoped_work_unit_with_env(
            &store,
            "work-unit-163-scope",
            &github_environment("tesserine/runa"),
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

        let result = validate_scoped_work_unit_with_env(
            &store,
            "work-unit-163-scope-a",
            &sourcehut_environment(4),
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

        let result = validate_scoped_work_unit_with_env(
            &store,
            "work-unit-163-scope",
            &sourcehut_environment(5),
        );

        assert!(result.is_err());
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
