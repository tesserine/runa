use std::process::Command;

use runa_forge_capability::{
    ForgeConnector, ForgeError, ForgeOperation, ForgeToolSet, Handle, canonical_tool_set,
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Clone, Deserialize)]
pub struct GitHubConfig {
    pub repository: String,
    pub assignee: Option<String>,
    pub credentials: Option<CredentialConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CredentialConfig {
    pub env: Option<String>,
    pub command: Option<Vec<String>>,
}

pub struct GitHubConnector {
    config: GitHubConfig,
}

impl GitHubConnector {
    pub fn new(config: GitHubConfig) -> Self {
        Self { config }
    }

    pub fn from_value(value: &toml::Value) -> Result<Self, ForgeError> {
        let config = value.clone().try_into().map_err(|error| {
            ForgeError::new(format!("invalid github connector config: {error}"))
        })?;
        Ok(Self::new(config))
    }

    fn ticket_handle(&self, number: u64) -> Handle {
        Handle {
            id: format!("github:issue:{number}"),
            display: format!(
                "https://github.com/{}/issues/{number}",
                self.config.repository
            ),
        }
    }

    fn change_handle(&self, number: u64) -> Handle {
        Handle {
            id: format!("github:pull:{number}"),
            display: format!(
                "https://github.com/{}/pull/{number}",
                self.config.repository
            ),
        }
    }

    fn ticket_number_from_reference(&self, reference: &str) -> Result<u64, ForgeError> {
        let trimmed = reference.trim();
        if let Some(number) = trimmed.strip_prefix('#') {
            return parse_number(number, "ticket reference");
        }
        trimmed
            .rsplit(['/', ':'])
            .next()
            .ok_or_else(|| ForgeError::new("ticket reference is empty"))
            .and_then(|part| parse_number(part, "ticket reference"))
    }

    fn ticket_number_from_handle(&self, handle: &Value) -> Result<u64, ForgeError> {
        handle_id(handle)?
            .strip_prefix("github:issue:")
            .ok_or_else(|| ForgeError::new("handle does not belong to a github issue"))?
            .parse()
            .map_err(|_| ForgeError::new("github issue handle has invalid number"))
    }

    fn pull_number_from_handle(&self, handle: &Value) -> Result<u64, ForgeError> {
        handle_id(handle)?
            .strip_prefix("github:pull:")
            .ok_or_else(|| ForgeError::new("handle does not belong to a github pull request"))?
            .parse()
            .map_err(|_| ForgeError::new("github pull request handle has invalid number"))
    }

    fn gh_json(&self, args: &[&str]) -> Result<Value, ForgeError> {
        let output = self.gh(args)?;
        serde_json::from_str(&output)
            .map_err(|error| ForgeError::new(format!("gh returned invalid JSON: {error}")))
    }

    fn gh(&self, args: &[&str]) -> Result<String, ForgeError> {
        let mut command = Command::new("gh");
        command.args(args);
        if let Some(token) = resolve_token(self.config.credentials.as_ref())? {
            command.env("GH_TOKEN", token);
        }
        let output = command
            .output()
            .map_err(|error| ForgeError::new(format!("failed to run gh: {error}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ForgeError::new(format!("gh command failed: {stderr}")));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn read_ticket(&self, reference: &str) -> Result<Value, ForgeError> {
        let number = self.ticket_number_from_reference(reference)?;
        let path = format!("repos/{}/issues/{number}", self.config.repository);
        let issue = self.gh_json(&["api", &path])?;
        Ok(json!({
            "handle": self.ticket_handle(number),
            "title": issue.get("title").and_then(Value::as_str).unwrap_or_default(),
            "body": issue.get("body").and_then(Value::as_str).unwrap_or_default(),
            "state": issue.get("state").and_then(Value::as_str).unwrap_or_default(),
            "url": issue.get("html_url").and_then(Value::as_str).unwrap_or_default()
        }))
    }

    fn create_ticket(&self, title: &str, body: &str) -> Result<Value, ForgeError> {
        let path = format!("repos/{}/issues", self.config.repository);
        let issue = self.gh_json(&[
            "api",
            "-X",
            "POST",
            &path,
            "-f",
            &format!("title={title}"),
            "-f",
            &format!("body={body}"),
        ])?;
        let number = issue
            .get("number")
            .and_then(Value::as_u64)
            .ok_or_else(|| ForgeError::new("created issue response omitted number"))?;
        Ok(json!({
            "handle": self.ticket_handle(number),
            "title": issue.get("title").and_then(Value::as_str).unwrap_or(title),
            "body": issue.get("body").and_then(Value::as_str).unwrap_or(body),
            "state": issue.get("state").and_then(Value::as_str).unwrap_or("open"),
            "url": issue.get("html_url").and_then(Value::as_str).unwrap_or_default()
        }))
    }

    fn comment_issue(&self, number: u64, body: &str) -> Result<String, ForgeError> {
        let path = format!("repos/{}/issues/{number}/comments", self.config.repository);
        let result = self.gh_json(&["api", "-X", "POST", &path, "-f", &format!("body={body}")])?;
        Ok(result
            .get("html_url")
            .and_then(Value::as_str)
            .unwrap_or("github comment recorded")
            .to_string())
    }
}

impl ForgeConnector for GitHubConnector {
    fn provider(&self) -> &'static str {
        "github"
    }

    fn tool_set(&self) -> ForgeToolSet {
        canonical_tool_set("forge:github")
    }

    fn call(&self, operation: ForgeOperation, input: Value) -> Result<Value, ForgeError> {
        match operation {
            ForgeOperation::ReadTicket => self.read_ticket(required_str(&input, "reference")?),
            ForgeOperation::CreateTicket => self.create_ticket(
                required_str(&input, "title")?,
                required_str(&input, "body")?,
            ),
            ForgeOperation::ClaimWorkUnit => {
                let number =
                    self.ticket_number_from_handle(input.get("handle").unwrap_or(&Value::Null))?;
                if let Some(assignee) = self.config.assignee.as_ref() {
                    let path =
                        format!("repos/{}/issues/{number}/assignees", self.config.repository);
                    let _ = self.gh_json(&[
                        "api",
                        "-X",
                        "POST",
                        &path,
                        "-f",
                        &format!("assignees[]={assignee}"),
                    ])?;
                }
                let receipt = self.comment_issue(number, "Claimed by runa.")?;
                Ok(json!({"handle": self.ticket_handle(number), "receipt": receipt}))
            }
            ForgeOperation::RecordProgress => {
                let number =
                    self.ticket_number_from_handle(input.get("handle").unwrap_or(&Value::Null))?;
                let receipt = self.comment_issue(number, required_str(&input, "body")?)?;
                Ok(json!({"handle": self.ticket_handle(number), "receipt": receipt}))
            }
            ForgeOperation::DeliverChangeProposal => {
                let work_unit = input.get("work_unit").cloned().unwrap_or(Value::Null);
                let title = required_str(&input, "summary")?;
                let body = required_str(&input, "body")?;
                let branch = required_str(&input, "branch")?;
                let base = required_str(&input, "base")?;
                let commit = required_str(&input, "commit")?;
                let version = input.get("version").and_then(Value::as_u64).unwrap_or(1);
                let path = format!("repos/{}/pulls", self.config.repository);
                let pr = self.gh_json(&[
                    "api",
                    "-X",
                    "POST",
                    &path,
                    "-f",
                    &format!("title={title}"),
                    "-f",
                    &format!("body={body}"),
                    "-f",
                    &format!("head={branch}"),
                    "-f",
                    &format!("base={base}"),
                ])?;
                let number = pr.get("number").and_then(Value::as_u64).ok_or_else(|| {
                    ForgeError::new("created pull request response omitted number")
                })?;
                Ok(json!({
                    "work_unit": work_unit,
                    "change": self.change_handle(number),
                    "version": version,
                    "commit": commit,
                    "receipt": pr.get("html_url").and_then(Value::as_str).unwrap_or("github pull request created")
                }))
            }
            ForgeOperation::ReflectDisposition => {
                let work_unit = input.get("work_unit").cloned().unwrap_or(Value::Null);
                let change = input.get("change").cloned().unwrap_or(Value::Null);
                let number = self.pull_number_from_handle(&change)?;
                let disposition = required_str(&input, "disposition")?;
                let body = required_str(&input, "body")?;
                let receipt = self.comment_issue(number, &format!("{disposition}\n\n{body}"))?;
                Ok(
                    json!({"work_unit": work_unit, "change": change, "disposition": disposition, "receipt": receipt}),
                )
            }
            ForgeOperation::ApplyApprovedChange => {
                let work_unit = input.get("work_unit").cloned().unwrap_or(Value::Null);
                let change = input.get("change").cloned().unwrap_or(Value::Null);
                let number = self.pull_number_from_handle(&change)?;
                let commit = required_str(&input, "approved_commit")?;
                let path = format!("repos/{}/pulls/{number}/merge", self.config.repository);
                let result = self.gh_json(&[
                    "api",
                    "-X",
                    "PUT",
                    &path,
                    "-f",
                    &format!("sha={commit}"),
                    "-f",
                    "merge_method=merge",
                ])?;
                Ok(json!({
                    "work_unit": work_unit,
                    "change": change,
                    "applied_commit": result.get("sha").and_then(Value::as_str).unwrap_or(commit),
                    "receipt": result.get("message").and_then(Value::as_str).unwrap_or("github pull request merged")
                }))
            }
            ForgeOperation::CloseOut => {
                let work_unit = input.get("work_unit").cloned().unwrap_or(Value::Null);
                let number = self.ticket_number_from_handle(&work_unit)?;
                let completion = required_str(&input, "completion")?;
                let body = required_str(&input, "body")?;
                let receipt = self.comment_issue(number, body)?;
                let path = format!("repos/{}/issues/{number}", self.config.repository);
                let _ = self.gh_json(&["api", "-X", "PATCH", &path, "-f", "state=closed"])?;
                Ok(json!({"work_unit": work_unit, "completion": completion, "receipt": receipt}))
            }
        }
    }
}

fn required_str<'a>(value: &'a Value, key: &str) -> Result<&'a str, ForgeError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ForgeError::new(format!("missing string field: {key}")))
}

fn handle_id(handle: &Value) -> Result<&str, ForgeError> {
    handle
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| ForgeError::new("handle is missing id"))
}

fn parse_number(value: &str, label: &str) -> Result<u64, ForgeError> {
    value
        .parse()
        .map_err(|_| ForgeError::new(format!("{label} is not a number")))
}

fn resolve_token(config: Option<&CredentialConfig>) -> Result<Option<String>, ForgeError> {
    let Some(config) = config else {
        return Ok(None);
    };
    if let Some(env_name) = config.env.as_deref() {
        return std::env::var(env_name)
            .map(Some)
            .map_err(|_| ForgeError::new(format!("credential env var is not set: {env_name}")));
    }
    if let Some(command) = config.command.as_ref() {
        let (program, args) = command
            .split_first()
            .ok_or_else(|| ForgeError::new("credential command cannot be empty"))?;
        let output = Command::new(program).args(args).output().map_err(|error| {
            ForgeError::new(format!("failed to run credential command: {error}"))
        })?;
        if !output.status.success() {
            return Err(ForgeError::new("credential command failed"));
        }
        return Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
        ));
    }
    Ok(None)
}
