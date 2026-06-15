//! Authoritative forge-address schema helpers.
//!
//! The JSON Schema under `contracts/forge-address/` is the public contract.
//! This module is runa's Rust projection of the same contract: it validates
//! produced payloads/handles against the schema and performs the semantic
//! checks JSON Schema cannot express, such as computed work-unit identity.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

use serde::Deserialize;
use serde_json::{Value, json};

const FORGE_ADDRESS_SCHEMA: &str =
    include_str!("../../contracts/forge-address/forge-address.schema.json");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgeAddressError {
    message: String,
}

impl ForgeAddressError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ForgeAddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ForgeAddressError {}

#[derive(Debug, Deserialize)]
struct ProjectDocument {
    #[serde(default)]
    forge: ForgeTopology,
}

#[derive(Debug, Default, Deserialize)]
struct ForgeTopology {
    #[serde(default)]
    instances: Vec<Instance>,
    #[serde(default)]
    repositories: Vec<Repository>,
    #[serde(default)]
    trackers: Vec<Tracker>,
}

#[derive(Debug, Deserialize)]
struct Instance {
    name: String,
    #[serde(rename = "type")]
    instance_type: String,
    host: Option<String>,
    git_host: Option<String>,
    tracker_host: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Repository {
    name: String,
    instance: String,
    owner: String,
    repository: String,
}

#[derive(Debug, Deserialize)]
struct Tracker {
    name: String,
    #[serde(rename = "type")]
    tracker_type: String,
    instance: String,
    repository: Option<String>,
    owner: Option<String>,
    tracker: Option<String>,
    tracker_id: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedInstance {
    name: String,
    instance_type: String,
    host: Option<String>,
    git_host: Option<String>,
    tracker_host: Option<String>,
}

pub fn schema() -> Value {
    serde_json::from_str(FORGE_ADDRESS_SCHEMA).expect("forge-address schema is valid JSON")
}

pub fn validate_against_schema(value: &Value) -> Result<(), ForgeAddressError> {
    let schema = schema();
    let validator = jsonschema::validator_for(&schema).map_err(|error| {
        ForgeAddressError::new(format!("invalid forge-address schema: {error}"))
    })?;
    let errors: Vec<String> = validator
        .iter_errors(value)
        .map(|error| format!("{}: {}", error.instance_path(), error))
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(ForgeAddressError::new(format!(
            "forge-address contract violation: {}",
            errors.join("; ")
        )))
    }
}

pub fn validate_work_unit_handle(value: &Value) -> Result<(), ForgeAddressError> {
    validate_against_schema(value)?;
    let tracker_identity = required_string(value, "tracker_identity")?;
    let work_unit_identity = required_string(value, "work_unit_identity")?;
    let number = value
        .get("number")
        .and_then(Value::as_u64)
        .ok_or_else(|| ForgeAddressError::new("work-unit handle number must be an integer"))?;
    let expected = format!("{tracker_identity}#{number}");
    if work_unit_identity != expected {
        return Err(ForgeAddressError::new(format!(
            "work_unit_identity must be derived as {expected}"
        )));
    }
    Ok(())
}

pub fn work_unit_handle(tracker: &str, tracker_identity: &str, number: u64) -> Value {
    json!({
        "tracker": tracker,
        "tracker_identity": tracker_identity,
        "work_unit_identity": format!("{tracker_identity}#{number}"),
        "number": number
    })
}

pub fn resolve_project_payload(path: &Path) -> Result<Value, ForgeAddressError> {
    let content = std::fs::read_to_string(path).map_err(|error| {
        ForgeAddressError::new(format!("cannot read {}: {error}", path.display()))
    })?;
    let project: ProjectDocument = toml::from_str(&content).map_err(|error| {
        ForgeAddressError::new(format!("failed to parse {}: {error}", path.display()))
    })?;
    let payload = resolve_topology(project.forge)?;
    validate_against_schema(&payload)?;
    Ok(payload)
}

fn resolve_topology(topology: ForgeTopology) -> Result<Value, ForgeAddressError> {
    let mut instances = BTreeMap::new();
    let mut instance_values = Vec::new();
    for instance in topology.instances {
        let resolved = resolve_instance(instance)?;
        if instances
            .insert(resolved.name.clone(), resolved.clone())
            .is_some()
        {
            return Err(ForgeAddressError::new("duplicate forge instance name"));
        }
        instance_values.push(instance_value(&resolved));
    }

    let mut repositories = BTreeMap::new();
    let mut repository_values = Vec::new();
    for repository in topology.repositories {
        let instance = instances.get(&repository.instance).ok_or_else(|| {
            ForgeAddressError::new(format!(
                "repository `{}` references unknown instance `{}`",
                repository.name, repository.instance
            ))
        })?;
        let identity = repository_identity(instance, &repository.owner, &repository.repository)?;
        if repositories
            .insert(
                repository.name.clone(),
                (repository.instance.clone(), identity.clone()),
            )
            .is_some()
        {
            return Err(ForgeAddressError::new("duplicate repository name"));
        }
        repository_values.push(json!({
            "name": repository.name,
            "instance": repository.instance,
            "owner": canonical_owner(instance, &repository.owner),
            "repository": repository.repository,
            "identity": identity
        }));
    }

    let mut tracker_names = BTreeSet::new();
    let mut tracker_values = Vec::new();
    for tracker in topology.trackers {
        if !tracker_names.insert(tracker.name.clone()) {
            return Err(ForgeAddressError::new("duplicate tracker name"));
        }
        let instance = instances.get(&tracker.instance).ok_or_else(|| {
            ForgeAddressError::new(format!(
                "tracker `{}` references unknown instance `{}`",
                tracker.name, tracker.instance
            ))
        })?;
        match tracker.tracker_type.as_str() {
            "github" => {
                let repository_name = tracker.repository.ok_or_else(|| {
                    ForgeAddressError::new(format!(
                        "github tracker `{}` must reference a repository",
                        tracker.name
                    ))
                })?;
                let (repository_instance, identity) =
                    repositories.get(&repository_name).ok_or_else(|| {
                        ForgeAddressError::new(format!(
                            "github tracker `{}` references unknown repository `{repository_name}`",
                            tracker.name
                        ))
                    })?;
                if repository_instance != &tracker.instance {
                    return Err(ForgeAddressError::new(format!(
                        "github tracker `{}` and repository `{repository_name}` use different instances",
                        tracker.name
                    )));
                }
                tracker_values.push(json!({
                    "name": tracker.name,
                    "type": "github",
                    "instance": tracker.instance,
                    "repository": repository_name,
                    "identity": identity
                }));
            }
            "sourcehut" => {
                let owner = tracker.owner.ok_or_else(|| {
                    ForgeAddressError::new(format!(
                        "sourcehut tracker `{}` missing owner",
                        tracker.name
                    ))
                })?;
                let tracker_name = tracker.tracker.ok_or_else(|| {
                    ForgeAddressError::new(format!(
                        "sourcehut tracker `{}` missing tracker",
                        tracker.name
                    ))
                })?;
                let tracker_id = tracker.tracker_id.ok_or_else(|| {
                    ForgeAddressError::new(format!(
                        "sourcehut tracker `{}` missing tracker_id",
                        tracker.name
                    ))
                })?;
                let identity =
                    sourcehut_tracker_identity(instance, &owner, &tracker_name, tracker_id)?;
                tracker_values.push(json!({
                    "name": tracker.name,
                    "type": "sourcehut",
                    "instance": tracker.instance,
                    "owner": canonical_owner(instance, &owner),
                    "tracker": tracker_name,
                    "tracker_id": tracker_id,
                    "identity": identity
                }));
            }
            other => {
                return Err(ForgeAddressError::new(format!(
                    "unsupported tracker type `{other}`"
                )));
            }
        }
    }

    Ok(json!({
        "schema_version": "1.0.0",
        "instances": instance_values,
        "repositories": repository_values,
        "trackers": tracker_values
    }))
}

fn resolve_instance(instance: Instance) -> Result<ResolvedInstance, ForgeAddressError> {
    match instance.instance_type.as_str() {
        "github" => {
            let host = required(instance.host, "github instance host")?;
            Ok(ResolvedInstance {
                name: instance.name,
                instance_type: instance.instance_type,
                host: Some(host),
                git_host: None,
                tracker_host: None,
            })
        }
        "sourcehut" => {
            let git_host = required(instance.git_host, "sourcehut instance git_host")?;
            let tracker_host = required(instance.tracker_host, "sourcehut instance tracker_host")?;
            Ok(ResolvedInstance {
                name: instance.name,
                instance_type: instance.instance_type,
                host: None,
                git_host: Some(git_host),
                tracker_host: Some(tracker_host),
            })
        }
        other => Err(ForgeAddressError::new(format!(
            "unsupported forge instance type `{other}`"
        ))),
    }
}

fn instance_value(instance: &ResolvedInstance) -> Value {
    match instance.instance_type.as_str() {
        "github" => json!({
            "name": instance.name,
            "type": "github",
            "host": instance.host.as_ref().expect("github host resolved")
        }),
        "sourcehut" => json!({
            "name": instance.name,
            "type": "sourcehut",
            "git_host": instance.git_host.as_ref().expect("sourcehut git_host resolved"),
            "tracker_host": instance.tracker_host.as_ref().expect("sourcehut tracker_host resolved")
        }),
        _ => unreachable!("unsupported instance resolved"),
    }
}

fn repository_identity(
    instance: &ResolvedInstance,
    owner: &str,
    repository: &str,
) -> Result<String, ForgeAddressError> {
    match instance.instance_type.as_str() {
        "github" => Ok(format!(
            "github:{}/{}/{}",
            instance.host.as_ref().expect("github host resolved"),
            owner,
            repository
        )),
        "sourcehut" => Ok(format!(
            "sourcehut:{}/{}/{}",
            instance
                .git_host
                .as_ref()
                .expect("sourcehut git_host resolved"),
            canonical_sourcehut_owner(owner),
            repository
        )),
        other => Err(ForgeAddressError::new(format!(
            "repository cannot use instance type `{other}`"
        ))),
    }
}

fn sourcehut_tracker_identity(
    instance: &ResolvedInstance,
    owner: &str,
    tracker: &str,
    tracker_id: u64,
) -> Result<String, ForgeAddressError> {
    if instance.instance_type != "sourcehut" {
        return Err(ForgeAddressError::new(
            "sourcehut tracker must use a sourcehut instance",
        ));
    }
    Ok(format!(
        "sourcehut:{}/{}/{}:{}",
        instance
            .tracker_host
            .as_ref()
            .expect("sourcehut tracker_host resolved"),
        canonical_sourcehut_owner(owner),
        tracker,
        tracker_id
    ))
}

fn canonical_owner(instance: &ResolvedInstance, owner: &str) -> String {
    if instance.instance_type == "sourcehut" {
        canonical_sourcehut_owner(owner)
    } else {
        owner.to_string()
    }
}

fn canonical_sourcehut_owner(owner: &str) -> String {
    let bare = owner.trim_start_matches('~');
    format!("~{bare}")
}

fn required(value: Option<String>, name: &str) -> Result<String, ForgeAddressError> {
    value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ForgeAddressError::new(format!("missing required {name}")))
}

fn required_string<'a>(value: &'a Value, field: &str) -> Result<&'a str, ForgeAddressError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ForgeAddressError::new(format!("work-unit handle missing {field}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conformance_fixture() -> Value {
        serde_json::from_str(include_str!(
            "../../contracts/forge-address/conformance.json"
        ))
        .unwrap()
    }

    #[test]
    fn validates_computed_work_unit_identity() {
        let handle = work_unit_handle("runa", "github:github.example.com/tesserine/runa", 199);

        validate_work_unit_handle(&handle).unwrap();
    }

    #[test]
    fn rejects_stored_work_unit_identity_that_disagrees_with_parts() {
        let handle = json!({
            "tracker": "runa",
            "tracker_identity": "github:github.example.com/tesserine/runa",
            "work_unit_identity": "github:github.example.com/tesserine/runa#200",
            "number": 199
        });

        let error = validate_work_unit_handle(&handle).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("work_unit_identity must be derived"),
            "{error}"
        );
    }

    #[test]
    fn resolves_project_topology_into_schema_valid_payload() {
        let directory = tempfile::TempDir::new().unwrap();
        let project = directory.path().join("project.toml");
        std::fs::write(
            &project,
            r#"
[[forge.instances]]
name = "github-enterprise"
type = "github"
host = "github.example.com"

[[forge.instances]]
name = "srht"
type = "sourcehut"
git_host = "git.weforge.build"
tracker_host = "todo.weforge.build"

[[forge.repositories]]
name = "runa"
instance = "github-enterprise"
owner = "tesserine"
repository = "runa"

[[forge.repositories]]
name = "groundwork-srht"
instance = "srht"
owner = "operator"
repository = "groundwork"

[[forge.trackers]]
name = "runa"
type = "github"
instance = "github-enterprise"
repository = "runa"

[[forge.trackers]]
name = "groundwork"
type = "sourcehut"
instance = "srht"
owner = "operator"
tracker = "groundwork"
tracker_id = 4
"#,
        )
        .unwrap();

        let payload = resolve_project_payload(&project).unwrap();

        assert_eq!(payload["schema_version"], "1.0.0");
        assert_eq!(
            payload["trackers"][1]["identity"],
            "sourcehut:todo.weforge.build/~operator/groundwork:4"
        );
        validate_against_schema(&payload).unwrap();
    }

    #[test]
    fn rejects_sourcehut_instance_missing_required_host() {
        let directory = tempfile::TempDir::new().unwrap();
        let project = directory.path().join("project.toml");
        std::fs::write(
            &project,
            r#"
[[forge.instances]]
name = "srht"
type = "sourcehut"
git_host = "git.weforge.build"
"#,
        )
        .unwrap();

        let error = resolve_project_payload(&project).unwrap_err();

        assert!(error.to_string().contains("tracker_host"), "{error}");
    }

    #[test]
    fn conformance_fixture_valid_examples_pass_contract() {
        let fixture = conformance_fixture();
        for example in fixture["valid"]["payloads"].as_array().unwrap() {
            validate_against_schema(&example["value"]).unwrap_or_else(|error| {
                panic!("valid payload {} failed: {error}", example["name"])
            });
        }
        for example in fixture["valid"]["handles"].as_array().unwrap() {
            validate_work_unit_handle(&example["value"])
                .unwrap_or_else(|error| panic!("valid handle {} failed: {error}", example["name"]));
        }
    }

    #[test]
    fn conformance_fixture_invalid_examples_fail_contract() {
        let fixture = conformance_fixture();
        for example in fixture["invalid"]["payloads"].as_array().unwrap() {
            assert!(
                validate_against_schema(&example["value"]).is_err(),
                "invalid payload {} passed",
                example["name"]
            );
        }
        for example in fixture["invalid"]["handles"].as_array().unwrap() {
            assert!(
                validate_work_unit_handle(&example["value"]).is_err(),
                "invalid handle {} passed",
                example["name"]
            );
        }
    }
}
