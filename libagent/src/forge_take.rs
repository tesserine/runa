use std::fmt;
use std::fs;

use serde_json::{Value, json};

use crate::project::{ForgeConfig, LoadedProject};
use crate::store::StoreError;
use crate::validation::{ValidationError, validate_artifact};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedForge {
    Github {
        owner: String,
        name: String,
    },
    Sourcehut {
        owner: String,
        name: String,
        tracker_id: u64,
    },
}

#[derive(Debug)]
pub enum TakeError {
    MissingForgeField(&'static str),
    InvalidTrackerId(String),
    UnsupportedForgeType(String),
    MissingCredential(&'static str),
    MissingWorkUnitArtifactType,
    MissingResponseField(&'static str),
    ForgeApi(String),
    PullRequestNotWorkUnit(u64),
    EmptyIssueBody,
    MissingAcceptanceCriteria,
    Validation(ValidationError),
    Store(StoreError),
    Io(std::io::Error),
    Serialization(serde_json::Error),
}

impl fmt::Display for TakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TakeError::MissingForgeField(field) => write!(f, "missing [forge] {field}"),
            TakeError::InvalidTrackerId(value) => {
                write!(f, "invalid [forge] tracker_id '{value}'")
            }
            TakeError::UnsupportedForgeType(forge_type) => {
                write!(f, "unsupported forge type '{forge_type}'")
            }
            TakeError::MissingCredential(name) => {
                write!(f, "missing required credential environment variable {name}")
            }
            TakeError::MissingWorkUnitArtifactType => {
                write!(f, "methodology does not declare a work-unit artifact type")
            }
            TakeError::MissingResponseField(field) => {
                write!(f, "forge response is missing field {field}")
            }
            TakeError::ForgeApi(detail) => write!(f, "forge API error: {detail}"),
            TakeError::PullRequestNotWorkUnit(number) => {
                write!(
                    f,
                    "GitHub issue {number} is a pull request, not a work unit"
                )
            }
            TakeError::EmptyIssueBody => write!(f, "forge issue body is empty"),
            TakeError::MissingAcceptanceCriteria => {
                write!(
                    f,
                    "forge issue body has no recognized acceptance criteria list"
                )
            }
            TakeError::Validation(err) => write!(f, "{err}"),
            TakeError::Store(err) => write!(f, "{err}"),
            TakeError::Io(err) => write!(f, "{err}"),
            TakeError::Serialization(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for TakeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TakeError::Validation(err) => Some(err),
            TakeError::Store(err) => Some(err),
            TakeError::Io(err) => Some(err),
            TakeError::Serialization(err) => Some(err),
            _ => None,
        }
    }
}

impl From<StoreError> for TakeError {
    fn from(err: StoreError) -> Self {
        Self::Store(err)
    }
}

impl From<std::io::Error> for TakeError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<serde_json::Error> for TakeError {
    fn from(err: serde_json::Error) -> Self {
        Self::Serialization(err)
    }
}

pub trait ForgeFetcher {
    fn fetch_github_issue(
        &self,
        owner: &str,
        name: &str,
        id: u64,
        token: &str,
    ) -> Result<ForgeIssue, TakeError>;

    fn fetch_sourcehut_issue(
        &self,
        owner: &str,
        name: &str,
        tracker_id: u64,
        id: u64,
        token: &str,
    ) -> Result<ForgeIssue, TakeError>;
}

pub struct HttpForgeFetcher;

impl ForgeFetcher for HttpForgeFetcher {
    fn fetch_github_issue(
        &self,
        owner: &str,
        name: &str,
        id: u64,
        token: &str,
    ) -> Result<ForgeIssue, TakeError> {
        let url = format!("https://api.github.com/repos/{owner}/{name}/issues/{id}");
        let body = ureq::get(&url)
            .set("Accept", "application/vnd.github+json")
            .set("Authorization", &format!("Bearer {token}"))
            .set("X-GitHub-Api-Version", "2022-11-28")
            .call()
            .map_err(http_error)?
            .into_string()?;
        parse_github_issue(serde_json::from_str(&body)?)
    }

    fn fetch_sourcehut_issue(
        &self,
        owner: &str,
        name: &str,
        tracker_id: u64,
        id: u64,
        token: &str,
    ) -> Result<ForgeIssue, TakeError> {
        let body = json!({
            "query": SOURCEHUT_READ_TICKET_QUERY,
            "variables": {
                "trackerOwner": owner,
                "trackerName": name,
                "ticketId": id,
            },
        });
        let body = ureq::post("https://todo.sr.ht/query")
            .set("Accept", "application/json")
            .set("Content-Type", "application/json")
            .set("Authorization", &format!("Bearer {token}"))
            .send_string(&body.to_string())
            .map_err(http_error)?
            .into_string()?;
        parse_sourcehut_issue(serde_json::from_str(&body)?, tracker_id)
    }
}

const SOURCEHUT_READ_TICKET_QUERY: &str = r#"
query readTicket($trackerOwner: String!, $trackerName: String!, $ticketId: Int!) {
  user(username: $trackerOwner) {
    tracker(name: $trackerName) {
      id
      ticket(id: $ticketId) {
        id
        ref
        subject
        body
        status
        resolution
      }
    }
  }
}
"#;

fn http_error(err: ureq::Error) -> TakeError {
    match err {
        ureq::Error::Status(status, response) => {
            let body = response.into_string().unwrap_or_default();
            TakeError::ForgeApi(format!("HTTP {status}: {body}"))
        }
        ureq::Error::Transport(err) => TakeError::ForgeApi(err.to_string()),
    }
}

pub fn take_work_unit(loaded: &mut LoadedProject, id: u64) -> Result<String, TakeError> {
    take_work_unit_with_fetcher(loaded, id, &HttpForgeFetcher, |name| {
        std::env::var(name).ok()
    })
}

pub fn take_work_unit_with_fetcher(
    loaded: &mut LoadedProject,
    id: u64,
    fetcher: &impl ForgeFetcher,
    env: impl Fn(&str) -> Option<String>,
) -> Result<String, TakeError> {
    let environment = resolve_take_forge_environment(&loaded.config.forge, &env);
    let resolved = ResolvedForge::from_environment(&environment)?;
    let issue = match resolved {
        ResolvedForge::Github { owner, name } => {
            let token = required_credential(&env, &["GITHUB_TOKEN", "GH_TOKEN"])?;
            fetcher.fetch_github_issue(&owner, &name, id, &token)?
        }
        ResolvedForge::Sourcehut {
            owner,
            name,
            tracker_id,
        } => {
            let token = required_credential(&env, &["SOURCEHUT_TOKEN", "SRHT_TOKEN"])?;
            fetcher.fetch_sourcehut_issue(&owner, &name, tracker_id, id, &token)?
        }
    };
    let work_unit = issue.into_work_unit()?;
    let artifact_type = loaded
        .manifest
        .artifact_types
        .iter()
        .find(|artifact_type| artifact_type.name == "work-unit")
        .ok_or(TakeError::MissingWorkUnitArtifactType)?;
    validate_artifact(&work_unit, artifact_type).map_err(TakeError::Validation)?;

    let instance_id = id.to_string();
    let work_unit_dir = loaded.workspace_dir.join("work-unit");
    fs::create_dir_all(&work_unit_dir)?;
    let artifact_path = work_unit_dir.join(format!("{instance_id}.json"));
    let json = serde_json::to_string_pretty(&work_unit)?;
    fs::write(&artifact_path, format!("{json}\n"))?;
    loaded
        .store
        .record("work-unit", &instance_id, &artifact_path, &work_unit)?;

    Ok(instance_id)
}

fn resolve_take_forge_environment(
    config: &ForgeConfig,
    env: &impl Fn(&str) -> Option<String>,
) -> std::collections::HashMap<String, String> {
    let mut environment = std::collections::HashMap::new();
    for name in [
        "GROUNDWORK_FORGE_TYPE",
        "GROUNDWORK_FORGE_OWNER",
        "GROUNDWORK_FORGE_NAME",
        "GROUNDWORK_FORGE_TRACKER_ID",
    ] {
        if let Some(value) = env(name).filter(|value| !value.is_empty()) {
            environment.insert(name.to_string(), value);
        }
    }
    insert_config_forge_env(
        &mut environment,
        "GROUNDWORK_FORGE_TYPE",
        config.forge_type.as_deref(),
    );
    environment
        .entry("GROUNDWORK_FORGE_TYPE".to_string())
        .or_insert_with(|| "github".to_string());
    insert_config_forge_env(
        &mut environment,
        "GROUNDWORK_FORGE_OWNER",
        config.owner.as_deref(),
    );
    insert_config_forge_env(
        &mut environment,
        "GROUNDWORK_FORGE_NAME",
        config.name.as_deref(),
    );
    insert_config_forge_env(
        &mut environment,
        "GROUNDWORK_FORGE_TRACKER_ID",
        config.tracker_id.as_deref(),
    );
    environment
}

fn insert_config_forge_env(
    environment: &mut std::collections::HashMap<String, String>,
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

fn required_credential(
    env: &impl Fn(&str) -> Option<String>,
    names: &[&'static str],
) -> Result<String, TakeError> {
    for name in names {
        if let Some(value) = env(name).filter(|value| !value.is_empty()) {
            return Ok(value);
        }
    }
    Err(TakeError::MissingCredential(names[0]))
}

impl ResolvedForge {
    pub fn from_config(config: &ForgeConfig) -> Result<Self, TakeError> {
        let environment = crate::scoped_identity::resolve_forge_environment(config);
        Self::from_environment(&environment)
    }

    pub fn from_environment(
        environment: &std::collections::HashMap<String, String>,
    ) -> Result<Self, TakeError> {
        let forge_type = config_field(environment, "GROUNDWORK_FORGE_TYPE")
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("github")
            .to_ascii_lowercase();
        let owner =
            required_config_field(config_field(environment, "GROUNDWORK_FORGE_OWNER"), "owner")?;
        let name =
            required_config_field(config_field(environment, "GROUNDWORK_FORGE_NAME"), "name")?;

        match forge_type.as_str() {
            "github" => Ok(Self::Github { owner, name }),
            "sourcehut" => {
                let tracker_id = required_config_field(
                    config_field(environment, "GROUNDWORK_FORGE_TRACKER_ID"),
                    "tracker_id",
                )?;
                let tracker_id = tracker_id
                    .parse::<u64>()
                    .map_err(|_| TakeError::InvalidTrackerId(tracker_id.clone()))?;
                Ok(Self::Sourcehut {
                    owner,
                    name,
                    tracker_id,
                })
            }
            other => Err(TakeError::UnsupportedForgeType(other.to_string())),
        }
    }
}

fn config_field<'a>(
    environment: &'a std::collections::HashMap<String, String>,
    key: &str,
) -> Option<&'a str> {
    environment.get(key).map(String::as_str)
}

fn required_config_field(value: Option<&str>, field: &'static str) -> Result<String, TakeError> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or(TakeError::MissingForgeField(field))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForgeHandle {
    Github { url: String, number: u64 },
    Sourcehut { tracker_id: u64, number: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgeIssue {
    pub title: String,
    pub body: String,
    pub handle: ForgeHandle,
}

impl ForgeIssue {
    pub fn into_work_unit(self) -> Result<Value, TakeError> {
        let description = self.body.trim().to_string();
        if description.is_empty() {
            return Err(TakeError::EmptyIssueBody);
        }
        let acceptance_criteria = extract_acceptance_criteria(&description)?;

        Ok(json!({
            "title": self.title,
            "description": description,
            "acceptance_criteria": acceptance_criteria,
            "handle": self.handle.into_json(),
        }))
    }
}

fn parse_github_issue(value: Value) -> Result<ForgeIssue, TakeError> {
    let number = required_u64(&value, "number")?;
    if value.get("pull_request").is_some() {
        return Err(TakeError::PullRequestNotWorkUnit(number));
    }
    Ok(ForgeIssue {
        title: required_string(&value, "title")?.to_string(),
        body: required_string(&value, "body")?.to_string(),
        handle: ForgeHandle::Github {
            url: required_string(&value, "html_url")?.to_string(),
            number,
        },
    })
}

fn parse_sourcehut_issue(value: Value, tracker_id: u64) -> Result<ForgeIssue, TakeError> {
    if let Some(errors) = value.get("errors").and_then(Value::as_array)
        && !errors.is_empty()
    {
        return Err(TakeError::ForgeApi(
            Value::Array(errors.clone()).to_string(),
        ));
    }
    let ticket = value
        .get("data")
        .and_then(|data| data.get("user"))
        .and_then(|user| user.get("tracker"))
        .and_then(|tracker| tracker.get("ticket"))
        .ok_or(TakeError::MissingResponseField("data.user.tracker.ticket"))?;
    Ok(ForgeIssue {
        title: required_string(ticket, "subject")?.to_string(),
        body: required_string(ticket, "body")?.to_string(),
        handle: ForgeHandle::Sourcehut {
            tracker_id,
            number: required_u64(ticket, "id")?,
        },
    })
}

fn required_string<'a>(value: &'a Value, field: &'static str) -> Result<&'a str, TakeError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or(TakeError::MissingResponseField(field))
}

fn required_u64(value: &Value, field: &'static str) -> Result<u64, TakeError> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .ok_or(TakeError::MissingResponseField(field))
}

impl ForgeHandle {
    fn into_json(self) -> Value {
        match self {
            Self::Github { url, number } => json!({
                "forge_tag": "github",
                "url": url,
                "number": number,
            }),
            Self::Sourcehut { tracker_id, number } => json!({
                "forge_tag": "sourcehut",
                "tracker_id": tracker_id,
                "number": number,
            }),
        }
    }
}

fn extract_acceptance_criteria(body: &str) -> Result<Vec<String>, TakeError> {
    let lines = body.lines().collect::<Vec<_>>();
    for (index, line) in lines.iter().enumerate() {
        if !is_acceptance_heading(line) {
            continue;
        }

        let mut criteria = Vec::new();
        for following in &lines[index + 1..] {
            let trimmed = following.trim();
            if trimmed.is_empty() {
                continue;
            }
            if looks_like_heading(trimmed) {
                break;
            }
            if let Some(item) = markdown_list_item(trimmed) {
                criteria.push(item.to_string());
            }
        }

        if !criteria.is_empty() {
            return Ok(criteria);
        }
    }

    Err(TakeError::MissingAcceptanceCriteria)
}

fn is_acceptance_heading(line: &str) -> bool {
    let heading = normalized_heading(line);
    matches!(
        heading.as_str(),
        "acceptance criteria" | "verification criteria" | "verification gate" | "what must be true"
    )
}

fn looks_like_heading(line: &str) -> bool {
    if markdown_list_item(line).is_some() {
        return false;
    }
    line.ends_with(':') || line.starts_with('#')
}

fn normalized_heading(line: &str) -> String {
    line.trim()
        .trim_start_matches('#')
        .trim()
        .trim_end_matches(':')
        .trim()
        .to_ascii_lowercase()
}

fn markdown_list_item(line: &str) -> Option<&str> {
    if let Some(item) = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| line.strip_prefix("+ "))
    {
        return Some(item.trim());
    }

    let (number, rest) = line.split_once(". ")?;
    if number.chars().all(|ch| ch.is_ascii_digit()) {
        Some(rest.trim())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::ForgeConfig;
    use crate::store::ValidationStatus;
    use crate::test_helpers::write_methodology;
    use std::cell::RefCell;
    use std::fs;
    use std::path::Path;

    #[test]
    fn github_issue_with_verification_gate_becomes_work_unit() {
        let body = r#"
What must be true:

This section has prose but no list.

Verification gate:

1. `runa take 58` writes the work unit.
2. `runa go --work-unit 58` can advance it.
"#;
        let issue = ForgeIssue {
            title: "Add runa take".to_string(),
            body: body.to_string(),
            handle: ForgeHandle::Github {
                url: "https://github.com/tesserine/runa/issues/58".to_string(),
                number: 58,
            },
        };

        let work_unit = issue.into_work_unit().unwrap();

        assert_eq!(work_unit["title"], "Add runa take");
        assert_eq!(
            work_unit["acceptance_criteria"],
            json!([
                "`runa take 58` writes the work unit.",
                "`runa go --work-unit 58` can advance it."
            ])
        );
        assert_eq!(
            work_unit["handle"],
            json!({
                "forge_tag": "github",
                "url": "https://github.com/tesserine/runa/issues/58",
                "number": 58
            })
        );
    }

    #[test]
    fn sourcehut_issue_uses_configured_tracker_id_in_handle() {
        let body = r#"
Acceptance criteria:

- Fetches from SourceHut.
"#;
        let issue = ForgeIssue {
            title: "SourceHut take".to_string(),
            body: body.to_string(),
            handle: ForgeHandle::Sourcehut {
                tracker_id: 4,
                number: 58,
            },
        };

        let work_unit = issue.into_work_unit().unwrap();

        assert_eq!(
            work_unit["handle"],
            json!({
                "forge_tag": "sourcehut",
                "tracker_id": 4,
                "number": 58
            })
        );
    }

    #[test]
    fn resolved_forge_requires_configured_owner_and_name() {
        let config = ForgeConfig {
            forge_type: Some("github".to_string()),
            owner: Some("tesserine".to_string()),
            name: Some("runa".to_string()),
            tracker_id: None,
        };

        let resolved = ResolvedForge::from_config(&config).unwrap();

        assert_eq!(
            resolved,
            ResolvedForge::Github {
                owner: "tesserine".to_string(),
                name: "runa".to_string()
            }
        );
    }

    #[test]
    fn github_parser_rejects_pull_request_payloads() {
        let payload = json!({
            "number": 58,
            "title": "A pull request",
            "body": "Acceptance criteria:\n\n- Never fetched as work.",
            "html_url": "https://github.com/tesserine/runa/pull/58",
            "pull_request": {}
        });

        let err = parse_github_issue(payload).unwrap_err();

        assert!(matches!(err, TakeError::PullRequestNotWorkUnit(58)));
    }

    #[test]
    fn sourcehut_parser_uses_configured_tracker_id_in_handle() {
        let payload = json!({
            "data": {
                "user": {
                    "tracker": {
                        "id": "opaque-tracker-id",
                        "ticket": {
                            "id": 58,
                            "ref": "operator/weforge#58",
                            "subject": "SourceHut work",
                            "body": "Verification gate:\n\n1. Fetch SourceHut issue.",
                            "status": "reported",
                            "resolution": null
                        }
                    }
                }
            }
        });

        let issue = parse_sourcehut_issue(payload, 4).unwrap();

        assert_eq!(
            issue,
            ForgeIssue {
                title: "SourceHut work".to_string(),
                body: "Verification gate:\n\n1. Fetch SourceHut issue.".to_string(),
                handle: ForgeHandle::Sourcehut {
                    tracker_id: 4,
                    number: 58
                }
            }
        );
    }

    #[test]
    fn take_work_unit_writes_valid_artifact_and_returns_id() {
        let dir = tempfile::tempdir().unwrap();
        let project_dir = initialized_project(dir.path(), github_forge_config());
        let mut loaded = crate::project::load(&project_dir, None).unwrap();
        let fetcher = FakeFetcher::github(ForgeIssue {
            title: "Fetch work".to_string(),
            body: "Verification gate:\n\n1. Write a work unit.".to_string(),
            handle: ForgeHandle::Github {
                url: "https://github.com/tesserine/runa/issues/58".to_string(),
                number: 58,
            },
        });

        let id = take_work_unit_with_fetcher(&mut loaded, 58, &fetcher, token_env).unwrap();

        assert_eq!(id, "58");
        let artifact_path = project_dir.join(".runa/workspace/work-unit/58.json");
        let artifact: Value = serde_json::from_str(&fs::read_to_string(&artifact_path).unwrap())
            .expect("work-unit JSON");
        assert_eq!(artifact["title"], "Fetch work");
        assert_eq!(
            artifact["acceptance_criteria"],
            json!(["Write a work unit."])
        );
        assert_eq!(
            loaded
                .store
                .get("work-unit", "58")
                .map(|state| &state.status),
            Some(&ValidationStatus::Valid)
        );
    }

    #[test]
    fn take_work_unit_reports_missing_github_credential_before_fetching() {
        let dir = tempfile::tempdir().unwrap();
        let project_dir = initialized_project(dir.path(), github_forge_config());
        let mut loaded = crate::project::load(&project_dir, None).unwrap();
        let fetcher = FakeFetcher::github(ForgeIssue {
            title: "Fetch work".to_string(),
            body: "Acceptance criteria:\n\n- Write a work unit.".to_string(),
            handle: ForgeHandle::Github {
                url: "https://github.com/tesserine/runa/issues/58".to_string(),
                number: 58,
            },
        });

        let err = take_work_unit_with_fetcher(&mut loaded, 58, &fetcher, |_| None).unwrap_err();

        assert!(matches!(err, TakeError::MissingCredential("GITHUB_TOKEN")));
        assert_eq!(*fetcher.calls.borrow(), 0);
    }

    #[test]
    fn take_work_unit_uses_groundwork_forge_environment_over_config() {
        let dir = tempfile::tempdir().unwrap();
        let project_dir = initialized_project(dir.path(), github_forge_config());
        let mut loaded = crate::project::load(&project_dir, None).unwrap();
        let fetcher = FakeFetcher::github(ForgeIssue {
            title: "Fetch work".to_string(),
            body: "Acceptance criteria:\n\n- Write a work unit.".to_string(),
            handle: ForgeHandle::Github {
                url: "https://github.com/env-owner/env-name/issues/58".to_string(),
                number: 58,
            },
        })
        .expect_repository("env-owner", "env-name");

        let id =
            take_work_unit_with_fetcher(&mut loaded, 58, &fetcher, token_and_forge_env).unwrap();

        assert_eq!(id, "58");
        assert_eq!(*fetcher.calls.borrow(), 1);
    }

    struct FakeFetcher {
        issue: ForgeIssue,
        calls: RefCell<usize>,
        expected_owner: &'static str,
        expected_name: &'static str,
    }

    impl FakeFetcher {
        fn github(issue: ForgeIssue) -> Self {
            Self {
                issue,
                calls: RefCell::new(0),
                expected_owner: "tesserine",
                expected_name: "runa",
            }
        }

        fn expect_repository(mut self, owner: &'static str, name: &'static str) -> Self {
            self.expected_owner = owner;
            self.expected_name = name;
            self
        }
    }

    impl ForgeFetcher for FakeFetcher {
        fn fetch_github_issue(
            &self,
            owner: &str,
            name: &str,
            id: u64,
            token: &str,
        ) -> Result<ForgeIssue, TakeError> {
            *self.calls.borrow_mut() += 1;
            assert_eq!(owner, self.expected_owner);
            assert_eq!(name, self.expected_name);
            assert_eq!(id, 58);
            assert_eq!(token, "secret");
            Ok(self.issue.clone())
        }

        fn fetch_sourcehut_issue(
            &self,
            _owner: &str,
            _name: &str,
            _tracker_id: u64,
            _id: u64,
            _token: &str,
        ) -> Result<ForgeIssue, TakeError> {
            unreachable!("github test should not fetch SourceHut")
        }
    }

    fn token_env(name: &str) -> Option<String> {
        (name == "GITHUB_TOKEN").then(|| "secret".to_string())
    }

    fn token_and_forge_env(name: &str) -> Option<String> {
        match name {
            "GITHUB_TOKEN" => Some("secret".to_string()),
            "GROUNDWORK_FORGE_OWNER" => Some("env-owner".to_string()),
            "GROUNDWORK_FORGE_NAME" => Some("env-name".to_string()),
            _ => None,
        }
    }

    fn github_forge_config() -> ForgeConfig {
        ForgeConfig {
            forge_type: Some("github".to_string()),
            owner: Some("tesserine".to_string()),
            name: Some("runa".to_string()),
            tracker_id: None,
        }
    }

    fn initialized_project(root: &Path, forge: ForgeConfig) -> std::path::PathBuf {
        let manifest_path = write_methodology(
            root,
            r#"
name = "groundwork"

[[artifact_types]]
name = "work-unit"

[[protocols]]
name = "take"
requires = ["work-unit"]
trigger = { type = "on_artifact", name = "work-unit" }
"#,
            &[(
                "work-unit",
                r#"{"type":"object","required":["title","description","acceptance_criteria"],"additionalProperties":false,"properties":{"title":{"type":"string","minLength":1},"description":{"type":"string","minLength":1},"acceptance_criteria":{"type":"array","minItems":1,"items":{"type":"string","minLength":1}},"handle":{"type":"object"}}}"#,
            )],
            &["take"],
        );
        let project_dir = root.join("project");
        fs::create_dir(&project_dir).unwrap();
        fs::create_dir(project_dir.join(".runa")).unwrap();
        let config = crate::project::Config {
            methodology_path: fs::canonicalize(manifest_path)
                .unwrap()
                .display()
                .to_string(),
            logging: crate::project::LoggingConfig::default(),
            agent: crate::project::AgentConfig::default(),
            transcript: crate::project::TranscriptConfig::default(),
            forge,
        };
        fs::write(
            project_dir.join(".runa/config.toml"),
            toml::to_string(&config).unwrap(),
        )
        .unwrap();
        fs::write(
            project_dir.join(".runa/state.toml"),
            "initialized_at = \"2026-01-01T00:00:00Z\"\nruna_version = \"0.2.0\"\n",
        )
        .unwrap();
        project_dir
    }
}
