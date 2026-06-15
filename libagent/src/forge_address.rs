//! Canonical forge-address contract.
//!
//! A forge-addressed value is a resource on a declared forge instance. The
//! instance is the sole home of the forge type and service hosts; resources
//! only point at an instance and carry resource-local coordinates.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const FORGE_ADDRESSES_ENV: &str = "RUNA_FORGE_ADDRESSES";
pub const PAYLOAD_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ForgeType {
    Github,
    Sourcehut,
}

impl ForgeType {
    pub fn parse(value: &str) -> Result<Self, ForgeAddressError> {
        match value.trim() {
            "github" => Ok(Self::Github),
            "sourcehut" => Ok(Self::Sourcehut),
            other => Err(ForgeAddressError::UnsupportedForgeType {
                forge_type: other.to_string(),
            }),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Sourcehut => "sourcehut",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServiceHosts {
    pub git: String,
    pub tracker: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ForgeInstance {
    pub id: String,
    #[serde(rename = "type")]
    pub forge_type: ForgeType,
    pub services: ServiceHosts,
}

impl ForgeInstance {
    pub fn identity_prefix(&self) -> String {
        match self.forge_type {
            ForgeType::Github => format!("github@{}", self.services.git),
            ForgeType::Sourcehut => format!(
                "sourcehut@git={},tracker={}",
                self.services.git, self.services.tracker
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ForgeRepository {
    pub id: String,
    pub instance: String,
    pub owner: String,
    pub name: String,
    pub identity: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ForgeTracker {
    pub id: String,
    pub instance: String,
    pub owner: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracker_id: Option<String>,
    pub repository: Option<String>,
    pub identity: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct ForgeProject {
    pub instances: Vec<ForgeInstance>,
    pub repositories: Vec<ForgeRepository>,
    pub trackers: Vec<ForgeTracker>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RawForges {
    #[serde(default)]
    pub instances: Vec<RawForgeInstance>,
    #[serde(default)]
    pub repositories: Vec<RawRepository>,
    #[serde(default)]
    pub trackers: Vec<RawTracker>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawForgeInstance {
    pub id: String,
    #[serde(rename = "type")]
    pub forge_type: String,
    pub host: Option<String>,
    pub git_host: Option<String>,
    pub tracker_host: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawRepository {
    pub id: String,
    pub instance: String,
    pub owner: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawTracker {
    pub id: String,
    pub instance: String,
    pub owner: String,
    pub name: String,
    pub tracker_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForgeAddressError {
    UnsupportedForgeType {
        forge_type: String,
    },
    MissingServiceHost {
        instance: String,
        forge_type: String,
        service: &'static str,
    },
    UnexpectedServiceHost {
        instance: String,
        field: &'static str,
    },
    DuplicateId {
        kind: &'static str,
        id: String,
    },
    UnknownInstance {
        resource_kind: &'static str,
        resource: String,
        instance: String,
    },
    MissingSourcehutTrackerId {
        tracker: String,
    },
    AmbiguousBareTrackerReference,
    UnknownRepositorySelector(String),
    UnknownTrackerSelector(String),
    MissingDeploymentRepository,
    AmbiguousDeploymentRepository,
    MalformedPayload(String),
}

impl fmt::Display for ForgeAddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ForgeAddressError::UnsupportedForgeType { forge_type } => write!(
                f,
                "unsupported forge type '{forge_type}'; expected one of: github, sourcehut"
            ),
            ForgeAddressError::MissingServiceHost {
                instance,
                forge_type,
                service,
            } => write!(
                f,
                "forge instance '{instance}' of type '{forge_type}' is missing required {service} service host"
            ),
            ForgeAddressError::UnexpectedServiceHost { instance, field } => write!(
                f,
                "forge instance '{instance}' declares unsupported service host field '{field}' for its type"
            ),
            ForgeAddressError::DuplicateId { kind, id } => {
                write!(f, "duplicate forge {kind} id '{id}'")
            }
            ForgeAddressError::UnknownInstance {
                resource_kind,
                resource,
                instance,
            } => write!(
                f,
                "forge {resource_kind} '{resource}' references unknown instance '{instance}'"
            ),
            ForgeAddressError::MissingSourcehutTrackerId { tracker } => write!(
                f,
                "sourcehut tracker '{tracker}' is missing required tracker_id"
            ),
            ForgeAddressError::AmbiguousBareTrackerReference => write!(
                f,
                "bare ticket reference is ambiguous because the project declares more than one tracker; qualify it with '<tracker>#<number>'"
            ),
            ForgeAddressError::UnknownRepositorySelector(selector) => {
                write!(
                    f,
                    "repository selector '{selector}' does not name a configured repository"
                )
            }
            ForgeAddressError::UnknownTrackerSelector(selector) => {
                write!(
                    f,
                    "tracker selector '{selector}' does not name a configured tracker"
                )
            }
            ForgeAddressError::MissingDeploymentRepository => write!(
                f,
                "transcript deployment requires [deployment].repository or exactly one configured repository"
            ),
            ForgeAddressError::AmbiguousDeploymentRepository => write!(
                f,
                "transcript deployment is ambiguous because the project declares multiple repositories; set [deployment].repository"
            ),
            ForgeAddressError::MalformedPayload(detail) => {
                write!(f, "malformed forge address payload: {detail}")
            }
        }
    }
}

impl std::error::Error for ForgeAddressError {}

impl ForgeProject {
    pub fn resolve(raw: RawForges) -> Result<Self, ForgeAddressError> {
        let mut instance_ids = HashSet::new();
        let mut instances = Vec::new();
        for raw_instance in raw.instances {
            if !instance_ids.insert(raw_instance.id.clone()) {
                return Err(ForgeAddressError::DuplicateId {
                    kind: "instance",
                    id: raw_instance.id,
                });
            }
            instances.push(resolve_instance(raw_instance)?);
        }
        let instance_by_id: HashMap<_, _> = instances
            .iter()
            .map(|instance| (instance.id.clone(), instance.clone()))
            .collect();

        let mut repository_ids = HashSet::new();
        let mut repositories = Vec::new();
        for raw_repository in raw.repositories {
            if !repository_ids.insert(raw_repository.id.clone()) {
                return Err(ForgeAddressError::DuplicateId {
                    kind: "repository",
                    id: raw_repository.id,
                });
            }
            let instance = instance_by_id
                .get(&raw_repository.instance)
                .ok_or_else(|| ForgeAddressError::UnknownInstance {
                    resource_kind: "repository",
                    resource: raw_repository.id.clone(),
                    instance: raw_repository.instance.clone(),
                })?;
            repositories.push(ForgeRepository {
                identity: format!(
                    "{}/repo/{}/{}",
                    instance.identity_prefix(),
                    raw_repository.owner,
                    raw_repository.name
                ),
                id: raw_repository.id,
                instance: raw_repository.instance,
                owner: raw_repository.owner,
                name: raw_repository.name,
            });
        }

        let mut tracker_ids = HashSet::new();
        let mut trackers = Vec::new();
        for repository in &repositories {
            let instance = instance_by_id
                .get(&repository.instance)
                .expect("repository instance was validated");
            if instance.forge_type == ForgeType::Github {
                if !tracker_ids.insert(repository.id.clone()) {
                    return Err(ForgeAddressError::DuplicateId {
                        kind: "tracker",
                        id: repository.id.clone(),
                    });
                }
                trackers.push(ForgeTracker {
                    id: repository.id.clone(),
                    instance: repository.instance.clone(),
                    owner: repository.owner.clone(),
                    name: repository.name.clone(),
                    tracker_id: None,
                    repository: Some(repository.id.clone()),
                    identity: format!(
                        "{}/tracker/{}/{}",
                        instance.identity_prefix(),
                        repository.owner,
                        repository.name
                    ),
                });
            }
        }
        for raw_tracker in raw.trackers {
            if !tracker_ids.insert(raw_tracker.id.clone()) {
                return Err(ForgeAddressError::DuplicateId {
                    kind: "tracker",
                    id: raw_tracker.id,
                });
            }
            let instance = instance_by_id.get(&raw_tracker.instance).ok_or_else(|| {
                ForgeAddressError::UnknownInstance {
                    resource_kind: "tracker",
                    resource: raw_tracker.id.clone(),
                    instance: raw_tracker.instance.clone(),
                }
            })?;
            let tracker_id = match instance.forge_type {
                ForgeType::Github => None,
                ForgeType::Sourcehut => Some(raw_tracker.tracker_id.clone().ok_or_else(|| {
                    ForgeAddressError::MissingSourcehutTrackerId {
                        tracker: raw_tracker.id.clone(),
                    }
                })?),
            };
            let identity = match instance.forge_type {
                ForgeType::Github => {
                    format!(
                        "{}/tracker/{}/{}",
                        instance.identity_prefix(),
                        raw_tracker.owner,
                        raw_tracker.name
                    )
                }
                ForgeType::Sourcehut => format!(
                    "{}/tracker/{}/{}/{}",
                    instance.identity_prefix(),
                    raw_tracker.owner,
                    raw_tracker.name,
                    tracker_id.as_deref().unwrap_or_default()
                ),
            };
            trackers.push(ForgeTracker {
                id: raw_tracker.id,
                instance: raw_tracker.instance,
                owner: raw_tracker.owner,
                name: raw_tracker.name,
                tracker_id,
                repository: None,
                identity,
            });
        }

        Ok(Self {
            instances,
            repositories,
            trackers,
        })
    }

    pub fn payload_value(&self) -> Value {
        json!({
            "schema_version": PAYLOAD_SCHEMA_VERSION,
            "instances": self.instances,
            "repositories": self.repositories,
            "trackers": self.trackers,
        })
    }

    pub fn payload_json(&self) -> Result<String, ForgeAddressError> {
        serde_json::to_string(&self.payload_value())
            .map_err(|error| ForgeAddressError::MalformedPayload(error.to_string()))
    }

    pub fn repository(
        &self,
        selector: Option<&str>,
    ) -> Result<&ForgeRepository, ForgeAddressError> {
        match selector {
            Some(selector) => self
                .repositories
                .iter()
                .find(|repository| repository.id == selector)
                .ok_or_else(|| ForgeAddressError::UnknownRepositorySelector(selector.to_string())),
            None if self.repositories.len() == 1 => Ok(&self.repositories[0]),
            None => Err(ForgeAddressError::UnknownRepositorySelector(
                "<missing>".to_string(),
            )),
        }
    }

    pub fn tracker(&self, selector: Option<&str>) -> Result<&ForgeTracker, ForgeAddressError> {
        match selector {
            Some(selector) => self
                .trackers
                .iter()
                .find(|tracker| tracker.id == selector)
                .ok_or_else(|| ForgeAddressError::UnknownTrackerSelector(selector.to_string())),
            None if self.trackers.len() == 1 => Ok(&self.trackers[0]),
            None if self.trackers.is_empty() => Err(ForgeAddressError::UnknownTrackerSelector(
                "<missing>".to_string(),
            )),
            None => Err(ForgeAddressError::AmbiguousBareTrackerReference),
        }
    }

    pub fn instance(&self, id: &str) -> Option<&ForgeInstance> {
        self.instances.iter().find(|instance| instance.id == id)
    }

    pub fn deployment_identity(&self, selector: Option<&str>) -> Result<String, ForgeAddressError> {
        Ok(self.repository(selector)?.identity.clone())
    }

    pub fn work_unit_identity(
        &self,
        tracker_selector: Option<&str>,
        number: u64,
    ) -> Result<String, ForgeAddressError> {
        Ok(format!(
            "{}#{number}",
            self.tracker(tracker_selector)?.identity
        ))
    }

    pub fn tracker_by_identity(&self, identity: &str) -> Option<&ForgeTracker> {
        self.trackers
            .iter()
            .find(|tracker| tracker.identity == identity)
    }
}

pub fn forge_type_from_handle(
    handle: &Map<String, Value>,
) -> Result<Option<ForgeType>, ForgeAddressError> {
    handle
        .get("forge_tag")
        .and_then(Value::as_str)
        .map(ForgeType::parse)
        .transpose()
}

pub fn tracker_identity_from_handle(
    handle: &Map<String, Value>,
) -> Result<Option<String>, ForgeAddressError> {
    let _ = forge_type_from_handle(handle)?;
    Ok(handle
        .get("tracker_identity")
        .and_then(Value::as_str)
        .map(str::to_string))
}

pub fn work_unit_identity_from_handle(
    handle: &Map<String, Value>,
) -> Result<Option<String>, ForgeAddressError> {
    let _ = forge_type_from_handle(handle)?;
    if let Some(identity) = handle.get("work_unit_identity").and_then(Value::as_str) {
        return Ok(Some(identity.to_string()));
    }
    let Some(tracker_identity) = handle.get("tracker_identity").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some(number) = handle.get("number").and_then(Value::as_u64) else {
        return Ok(None);
    };
    Ok(Some(format!("{tracker_identity}#{number}")))
}

fn resolve_instance(raw: RawForgeInstance) -> Result<ForgeInstance, ForgeAddressError> {
    let forge_type = ForgeType::parse(&raw.forge_type)?;
    let services = match forge_type {
        ForgeType::Github => {
            if raw.git_host.is_some() {
                return Err(ForgeAddressError::UnexpectedServiceHost {
                    instance: raw.id,
                    field: "git_host",
                });
            }
            if raw.tracker_host.is_some() {
                return Err(ForgeAddressError::UnexpectedServiceHost {
                    instance: raw.id,
                    field: "tracker_host",
                });
            }
            let host = raw
                .host
                .ok_or_else(|| ForgeAddressError::MissingServiceHost {
                    instance: raw.id.clone(),
                    forge_type: forge_type.as_str().to_string(),
                    service: "shared",
                })?;
            ServiceHosts {
                git: host.clone(),
                tracker: host,
            }
        }
        ForgeType::Sourcehut => {
            if raw.host.is_some() {
                return Err(ForgeAddressError::UnexpectedServiceHost {
                    instance: raw.id,
                    field: "host",
                });
            }
            ServiceHosts {
                git: raw
                    .git_host
                    .ok_or_else(|| ForgeAddressError::MissingServiceHost {
                        instance: raw.id.clone(),
                        forge_type: forge_type.as_str().to_string(),
                        service: "git",
                    })?,
                tracker: raw
                    .tracker_host
                    .ok_or_else(|| ForgeAddressError::MissingServiceHost {
                        instance: raw.id.clone(),
                        forge_type: forge_type.as_str().to_string(),
                        service: "tracker",
                    })?,
            }
        }
    };
    Ok(ForgeInstance {
        id: raw.id,
        forge_type,
        services,
    })
}

pub fn reject_legacy_environment() -> Result<(), ForgeAddressError> {
    let legacy = [
        "RUNA_FORGE_TYPE",
        "RUNA_FORGE_OWNER",
        "RUNA_FORGE_NAME",
        "RUNA_FORGE_TRACKER_ID",
        "GROUNDWORK_FORGE_ENDPOINT",
    ];
    let present: Vec<_> = legacy
        .into_iter()
        .filter(|name| std::env::var(name).is_ok_and(|value| !value.is_empty()))
        .collect();
    if present.is_empty() {
        return Ok(());
    }
    Err(ForgeAddressError::MalformedPayload(format!(
        "legacy forge environment variable(s) {} are no longer accepted; configure forge instances and resources in .runa/project.toml",
        present.join(", ")
    )))
}

pub fn payload_env(project: &ForgeProject) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    if let Ok(payload) = project.payload_json() {
        env.insert(FORGE_ADDRESSES_ENV.to_string(), payload);
    }
    env
}
