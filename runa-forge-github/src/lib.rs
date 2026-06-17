use std::fmt;
use std::sync::{Arc, Mutex};

use runa_forge_contract::{ForgeOperation, ForgeToolSet, Handle, forge_tool_set};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHubConfig {
    pub owner: String,
    pub repo: String,
    pub api_base_url: String,
    pub web_base_url: String,
    pub assignee: Option<String>,
    pub credential_env: Option<String>,
    pub credential_command: Option<Vec<String>>,
}

impl GitHubConfig {
    pub fn new(owner: impl Into<String>, repo: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            repo: repo.into(),
            api_base_url: "https://api.github.com".to_owned(),
            web_base_url: "https://github.com".to_owned(),
            assignee: None,
            credential_env: None,
            credential_command: None,
        }
    }

    fn repo_path(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderRequest {
    pub summary: String,
    pub method: String,
    pub path: String,
    pub body: Value,
}

#[derive(Clone, Debug, Default)]
pub struct RecordingGitHubTransport {
    requests: Arc<Mutex<Vec<ProviderRequest>>>,
}

impl RecordingGitHubTransport {
    pub fn record(&self, request: ProviderRequest) {
        self.requests
            .lock()
            .expect("request log poisoned")
            .push(request);
    }

    pub fn take_requests(&self) -> Vec<ProviderRequest> {
        std::mem::take(&mut *self.requests.lock().expect("request log poisoned"))
    }
}

#[derive(Clone, Debug)]
pub struct GitHubConnector {
    config: GitHubConfig,
    transport: RecordingGitHubTransport,
}

impl GitHubConnector {
    pub fn new_for_test(config: GitHubConfig, transport: RecordingGitHubTransport) -> Self {
        Self { config, transport }
    }

    pub fn config(&self) -> &GitHubConfig {
        &self.config
    }

    pub fn tool_set(&self) -> ForgeToolSet {
        forge_tool_set("github")
    }

    pub fn resolve_reference(&self, reference: &str) -> Result<Handle, ForgeGitHubError> {
        let issue = self.parse_ticket_number(reference)?;
        Ok(self.work_unit_handle(issue))
    }

    pub fn validate_work_unit_handle_id(&self, handle_id: &str) -> Result<u64, ForgeGitHubError> {
        let prefix = format!("github:{}:issue:", self.config.repo_path());
        let Some(number) = handle_id.strip_prefix(&prefix) else {
            return Err(ForgeGitHubError::OutOfScope(handle_id.to_owned()));
        };
        parse_positive_number(number)
    }

    pub fn call(&self, operation: ForgeOperation, input: Value) -> Result<Value, ForgeGitHubError> {
        match operation {
            ForgeOperation::ReadTicket => self.read_ticket(&input),
            ForgeOperation::CreateTicket => self.create_ticket(&input),
            ForgeOperation::ClaimWorkUnit => self.claim_work_unit(&input),
            ForgeOperation::RecordProgress => self.record_progress(&input),
            ForgeOperation::DeliverChangeProposal => self.deliver_change_proposal(&input),
            ForgeOperation::ReflectDisposition => self.reflect_disposition(&input),
            ForgeOperation::ApplyApprovedChange => self.apply_approved_change(&input),
            ForgeOperation::CloseOut => self.close_out(&input),
        }
    }

    fn read_ticket(&self, input: &Value) -> Result<Value, ForgeGitHubError> {
        let reference = input
            .get("reference")
            .and_then(Value::as_str)
            .ok_or_else(|| ForgeGitHubError::InvalidInput("missing reference".to_owned()))?;
        let handle = self.resolve_reference(reference)?;
        let issue = self.validate_work_unit_handle_id(&handle.id)?;
        self.record("GET", issue_path(&self.config, issue), json!({}));
        Ok(json!({
            "handle": handle,
            "title": "",
            "body": "",
            "state": "open",
            "url": self.issue_url(issue)
        }))
    }

    fn create_ticket(&self, input: &Value) -> Result<Value, ForgeGitHubError> {
        let title = string_field(input, "title")?;
        let body = input
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let issue = 0;
        let path = format!("repos/{}/issues", self.config.repo_path());
        self.record(
            "POST",
            path,
            json!({
                "title": title,
                "body": body,
                "labels": input.get("labels").cloned().unwrap_or_else(|| json!([]))
            }),
        );
        Ok(json!({
            "handle": self.work_unit_handle(issue),
            "title": title,
            "body": body,
            "state": "open",
            "url": self.issue_url(issue)
        }))
    }

    fn claim_work_unit(&self, input: &Value) -> Result<Value, ForgeGitHubError> {
        let (issue, handle) = self.work_unit_from_input(input)?;
        let assignee = input
            .get("claimant")
            .and_then(Value::as_str)
            .or(self.config.assignee.as_deref())
            .unwrap_or("core");
        self.record(
            "PATCH",
            issue_path(&self.config, issue),
            json!({ "assignees": [assignee] }),
        );
        Ok(effect(handle, "claimed", self.issue_url(issue)))
    }

    fn record_progress(&self, input: &Value) -> Result<Value, ForgeGitHubError> {
        let (issue, handle) = self.work_unit_from_input(input)?;
        let body = string_field(input, "body")?;
        let path = format!("{}/comments", issue_path(&self.config, issue));
        self.record("POST", path, json!({ "body": body }));
        Ok(effect(handle, "progress-recorded", self.issue_url(issue)))
    }

    fn deliver_change_proposal(&self, input: &Value) -> Result<Value, ForgeGitHubError> {
        let (issue, handle) = self.work_unit_from_input(input)?;
        let branch = string_field(input, "branch")?;
        let commit = string_field(input, "commit")?;
        let base = string_field(input, "base")?;
        let summary = string_field(input, "summary")?;
        let body = input
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let version = string_field(input, "version")?;
        let path = format!("repos/{}/pulls", self.config.repo_path());
        self.record(
            "POST",
            path,
            json!({
                "head": branch,
                "base": base,
                "title": summary,
                "body": body,
                "issue": issue
            }),
        );
        let change = Handle::new(
            format!(
                "github:{}:issue:{issue}:pull:0:v{version}",
                self.config.repo_path()
            ),
            format!("github:{}#{}!0@{}", self.config.repo_path(), issue, version),
        );
        Ok(json!({
            "change": change,
            "work_unit": handle,
            "commit": commit,
            "version": version,
            "url": self.pull_url(0)
        }))
    }

    fn reflect_disposition(&self, input: &Value) -> Result<Value, ForgeGitHubError> {
        let (issue, handle) = self.work_unit_from_input(input)?;
        let body = input
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let disposition = string_field(input, "disposition")?;
        let path = format!("{}/comments", issue_path(&self.config, issue));
        self.record(
            "POST",
            path,
            json!({ "body": format!("{disposition}: {body}") }),
        );
        Ok(effect(
            handle,
            "disposition-reflected",
            self.issue_url(issue),
        ))
    }

    fn apply_approved_change(&self, input: &Value) -> Result<Value, ForgeGitHubError> {
        let (issue, handle) = self.work_unit_from_input(input)?;
        let change = handle_field(input, "change")?;
        let commit = string_field(input, "approved_commit")?;
        self.record(
            "PUT",
            format!("repos/{}/pulls/0/merge", self.config.repo_path()),
            json!({ "sha": commit }),
        );
        Ok(json!({
            "work_unit": handle,
            "change": change,
            "applied_commit": commit,
            "status": "applied",
            "url": self.issue_url(issue)
        }))
    }

    fn close_out(&self, input: &Value) -> Result<Value, ForgeGitHubError> {
        let (issue, handle) = self.work_unit_from_input(input)?;
        let body = input
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or_default();
        self.record(
            "POST",
            format!("{}/comments", issue_path(&self.config, issue)),
            json!({ "body": body }),
        );
        self.record(
            "PATCH",
            issue_path(&self.config, issue),
            json!({ "state": "closed" }),
        );
        Ok(effect(handle, "closed-out", self.issue_url(issue)))
    }

    fn work_unit_from_input(&self, input: &Value) -> Result<(u64, Handle), ForgeGitHubError> {
        let handle = input
            .get("work_unit")
            .or_else(|| input.get("handle"))
            .ok_or_else(|| ForgeGitHubError::InvalidInput("missing work_unit handle".to_owned()))
            .and_then(parse_handle)?;
        let issue = self.validate_work_unit_handle_id(&handle.id)?;
        Ok((issue, handle))
    }

    fn parse_ticket_number(&self, reference: &str) -> Result<u64, ForgeGitHubError> {
        if let Some(rest) = reference.strip_prefix("github:") {
            let expected = format!("{}#", self.config.repo_path());
            let Some(number) = rest.strip_prefix(&expected) else {
                return Err(ForgeGitHubError::OutOfScope(reference.to_owned()));
            };
            return parse_positive_number(number);
        }

        if let Some(number) = reference.strip_prefix('#') {
            return parse_positive_number(number);
        }

        if let Some((repo_path, number)) = reference.split_once('#') {
            if repo_path == self.config.repo_path() {
                return parse_positive_number(number);
            }
            return Err(ForgeGitHubError::OutOfScope(reference.to_owned()));
        }

        parse_positive_number(reference)
    }

    fn work_unit_handle(&self, issue: u64) -> Handle {
        Handle::new(
            format!("github:{}:issue:{issue}", self.config.repo_path()),
            format!("github:{}#{issue}", self.config.repo_path()),
        )
    }

    fn issue_url(&self, issue: u64) -> String {
        format!(
            "{}/{}/issues/{issue}",
            self.config.web_base_url,
            self.config.repo_path()
        )
    }

    fn pull_url(&self, pull: u64) -> String {
        format!(
            "{}/{}/pull/{pull}",
            self.config.web_base_url,
            self.config.repo_path()
        )
    }

    fn record(&self, method: &str, path: String, body: Value) {
        self.transport.record(ProviderRequest {
            summary: format!("github {method} {path}"),
            method: method.to_owned(),
            path,
            body,
        });
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForgeGitHubError {
    InvalidInput(String),
    InvalidReference(String),
    OutOfScope(String),
}

impl fmt::Display for ForgeGitHubError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ForgeGitHubError::InvalidInput(message) => f.write_str(message),
            ForgeGitHubError::InvalidReference(reference) => {
                write!(f, "invalid GitHub reference `{reference}`")
            }
            ForgeGitHubError::OutOfScope(reference) => {
                write!(
                    f,
                    "GitHub reference `{reference}` is outside connector scope"
                )
            }
        }
    }
}

impl std::error::Error for ForgeGitHubError {}

fn issue_path(config: &GitHubConfig, issue: u64) -> String {
    format!("repos/{}/issues/{issue}", config.repo_path())
}

fn parse_positive_number(value: &str) -> Result<u64, ForgeGitHubError> {
    let number = value
        .parse::<u64>()
        .map_err(|_| ForgeGitHubError::InvalidReference(value.to_owned()))?;
    if number == 0 {
        return Err(ForgeGitHubError::InvalidReference(value.to_owned()));
    }
    Ok(number)
}

fn string_field<'a>(input: &'a Value, field: &str) -> Result<&'a str, ForgeGitHubError> {
    input
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| ForgeGitHubError::InvalidInput(format!("missing {field}")))
}

fn handle_field(input: &Value, field: &str) -> Result<Handle, ForgeGitHubError> {
    input
        .get(field)
        .ok_or_else(|| ForgeGitHubError::InvalidInput(format!("missing {field} handle")))
        .and_then(parse_handle)
}

fn parse_handle(value: &Value) -> Result<Handle, ForgeGitHubError> {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| ForgeGitHubError::InvalidInput("missing handle id".to_owned()))?;
    let display = value
        .get("display")
        .and_then(Value::as_str)
        .ok_or_else(|| ForgeGitHubError::InvalidInput("missing handle display".to_owned()))?;
    Ok(Handle::new(id, display))
}

fn effect(handle: Handle, status: &str, url: String) -> Value {
    json!({
        "work_unit": handle,
        "status": status,
        "url": url
    })
}
