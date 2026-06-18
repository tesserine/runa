use runa_forge_contract::{ForgeConnector, ForgeError, Handle, Operation};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

const GITHUB_USER_AGENT: &str = concat!("runa-forge-github/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubConfig {
    pub owner: String,
    pub repo: String,
    pub api_base: String,
    pub assignee: Option<String>,
    pub credential_env: Option<String>,
    pub credential_command: Option<Vec<String>>,
}

impl GithubConfig {
    fn credential(&self) -> Result<Option<String>, ForgeError> {
        if let Some(name) = self
            .credential_env
            .as_deref()
            .filter(|name| !name.is_empty())
            && let Ok(value) = std::env::var(name)
            && !value.is_empty()
        {
            return Ok(Some(value));
        }

        let Some(command) = self
            .credential_command
            .as_ref()
            .filter(|command| !command.is_empty())
        else {
            return Ok(None);
        };
        let Some((program, args)) = command.split_first() else {
            return Ok(None);
        };
        let output = std::process::Command::new(program)
            .args(args)
            .output()
            .map_err(|error| {
                ForgeError::Transport(format!("credential command failed: {error}"))
            })?;
        if !output.status.success() {
            return Err(ForgeError::Transport(format!(
                "credential command exited with status {}",
                output.status
            )));
        }
        let token = String::from_utf8(output.stdout)
            .map_err(|_| {
                ForgeError::Transport("credential command produced non-UTF-8 output".to_string())
            })?
            .trim()
            .to_string();
        Ok((!token.is_empty()).then_some(token))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderRequest {
    pub method: String,
    pub path: String,
    pub body: Option<Value>,
}

pub trait GithubTransport: Clone + Send + Sync + 'static {
    fn send(&self, config: &GithubConfig, request: ProviderRequest) -> Result<Value, ForgeError>;
}

#[derive(Debug, Clone, Default)]
pub struct GithubRecordingTransport {
    requests: Arc<Mutex<Vec<ProviderRequest>>>,
    responses: Arc<Mutex<VecDeque<Value>>>,
    repeating: Arc<Mutex<Option<Value>>>,
}

impl GithubRecordingTransport {
    pub fn with_response(response: Value) -> Self {
        let transport = Self::default();
        transport.responses.lock().unwrap().push_back(response);
        transport
    }

    pub fn with_repeating_response(response: Value) -> Self {
        let transport = Self::default();
        *transport.repeating.lock().unwrap() = Some(response);
        transport
    }

    pub fn requests(&self) -> Vec<ProviderRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl GithubTransport for GithubRecordingTransport {
    fn send(&self, _config: &GithubConfig, request: ProviderRequest) -> Result<Value, ForgeError> {
        self.requests.lock().unwrap().push(request);
        if let Some(response) = self.responses.lock().unwrap().pop_front() {
            return Ok(response);
        }
        if let Some(response) = self.repeating.lock().unwrap().clone() {
            return Ok(response);
        }
        Ok(json!({}))
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct GithubHttpTransport;

impl GithubTransport for GithubHttpTransport {
    fn send(&self, config: &GithubConfig, request: ProviderRequest) -> Result<Value, ForgeError> {
        let url = format!("{}{}", config.api_base.trim_end_matches('/'), request.path);
        let client = reqwest::blocking::Client::builder()
            .user_agent(GITHUB_USER_AGENT)
            .build()
            .map_err(|error| ForgeError::Transport(error.to_string()))?;
        let builder = match request.method.as_str() {
            "GET" => client.get(&url),
            "POST" => client.post(&url),
            "PATCH" => client.patch(&url),
            "PUT" => client.put(&url),
            other => {
                return Err(ForgeError::Transport(format!(
                    "unsupported HTTP method {other}"
                )));
            }
        };
        let builder = if let Some(token) = config.credential()? {
            builder.bearer_auth(token)
        } else {
            builder
        };
        let builder = if let Some(body) = request.body {
            builder.json(&body)
        } else {
            builder
        };
        let response = builder
            .send()
            .map_err(|error| ForgeError::Transport(error.to_string()))?;
        let status = response.status();
        let value = response
            .json::<Value>()
            .map_err(|error| ForgeError::ProviderResponse(error.to_string()))?;
        if !status.is_success() {
            return Err(ForgeError::Transport(format!("GitHub returned {status}")));
        }
        Ok(value)
    }
}

#[derive(Debug, Clone)]
pub struct GithubConnector<T> {
    config: GithubConfig,
    transport: T,
}

impl<T: GithubTransport> GithubConnector<T> {
    pub fn new(config: GithubConfig, transport: T) -> Self {
        Self { config, transport }
    }

    pub fn call(&self, operation: Operation, input: Value) -> Result<Value, ForgeError> {
        <Self as ForgeConnector>::call(self, operation, input)
    }

    fn repo_scope(&self) -> String {
        format!("{}/{}", self.config.owner, self.config.repo)
    }

    fn issue_handle(&self, number: u64) -> Handle {
        Handle {
            id: format!("github:{}:issue:{number}", self.repo_scope()),
            display: format!("{}#{number}", self.repo_scope()),
        }
    }

    fn pull_handle(&self, number: u64, version: u64) -> Handle {
        Handle {
            id: format!(
                "github:{}:pull:{number}:version:{version}",
                self.repo_scope()
            ),
            display: format!("{}#{number}", self.repo_scope()),
        }
    }

    fn send(&self, method: &str, path: String, body: Option<Value>) -> Result<Value, ForgeError> {
        self.transport.send(
            &self.config,
            ProviderRequest {
                method: method.to_string(),
                path,
                body,
            },
        )
    }
}

impl<T: GithubTransport> ForgeConnector for GithubConnector<T> {
    fn set_name(&self) -> &str {
        "github"
    }

    fn call(&self, operation: Operation, input: Value) -> Result<Value, ForgeError> {
        match operation {
            Operation::ReadTicket => self.read_ticket(input),
            Operation::CreateTicket => self.create_ticket(input),
            Operation::ClaimWorkUnit => self.claim_work_unit(input),
            Operation::RecordProgress => self.record_progress(input),
            Operation::DeliverChangeProposal => self.deliver_change_proposal(input),
            Operation::ReflectDisposition => self.reflect_disposition(input),
            Operation::ApplyApprovedChange => self.apply_approved_change(input),
            Operation::CloseOut => self.close_out(input),
        }
    }
}

impl<T: GithubTransport> GithubConnector<T> {
    fn read_ticket(&self, input: Value) -> Result<Value, ForgeError> {
        let reference = required_string(&input, "reference")?;
        let number = self.resolve_reference(reference)?;
        let response = self.send(
            "GET",
            format!(
                "/repos/{}/{}/issues/{number}",
                self.config.owner, self.config.repo
            ),
            None,
        )?;
        self.ticket_snapshot(number, &response)
    }

    fn create_ticket(&self, input: Value) -> Result<Value, ForgeError> {
        let title = required_string(&input, "title")?;
        let body = required_string(&input, "body")?;
        let response = self.send(
            "POST",
            format!("/repos/{}/{}/issues", self.config.owner, self.config.repo),
            Some(json!({ "title": title, "body": body })),
        )?;
        let number = response.get("number").and_then(Value::as_u64).unwrap_or(0);
        self.ticket_snapshot(number, &response)
    }

    fn claim_work_unit(&self, input: Value) -> Result<Value, ForgeError> {
        let number = self.issue_number(input.get("handle"))?;
        let assignee = self
            .config
            .assignee
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ForgeError::InvalidInput("github assignee is required".into()))?;
        let response = self.send(
            "POST",
            format!(
                "/repos/{}/{}/issues/{number}/assignees",
                self.config.owner, self.config.repo
            ),
            Some(json!({ "assignees": [assignee] })),
        )?;
        Ok(json!({ "handle": self.issue_handle(number), "receipt": receipt(response, "claimed") }))
    }

    fn record_progress(&self, input: Value) -> Result<Value, ForgeError> {
        let number = self.issue_number(input.get("handle"))?;
        let body = required_string(&input, "body")?;
        let response = self.send(
            "POST",
            format!(
                "/repos/{}/{}/issues/{number}/comments",
                self.config.owner, self.config.repo
            ),
            Some(json!({ "body": body })),
        )?;
        Ok(json!({ "handle": self.issue_handle(number), "receipt": receipt(response, "progress") }))
    }

    fn deliver_change_proposal(&self, input: Value) -> Result<Value, ForgeError> {
        let work_unit = input
            .get("work_unit")
            .ok_or_else(|| ForgeError::InvalidInput("work_unit is required".into()))?;
        self.issue_number(Some(work_unit))?;
        let branch = required_string(&input, "branch")?;
        let base = required_string(&input, "base")?;
        let summary = required_string(&input, "summary")?;
        let body = required_string(&input, "body")?;
        let commit = required_string(&input, "commit")?;
        let version = required_u64(&input, "version")?;
        let response = self.send(
            "POST",
            format!("/repos/{}/{}/pulls", self.config.owner, self.config.repo),
            Some(json!({ "title": summary, "head": branch, "base": base, "body": body })),
        )?;
        let number = response.get("number").and_then(Value::as_u64).unwrap_or(0);
        let delivered_commit = response_string(&response, &["/head/sha"])?;
        if delivered_commit != commit {
            return Err(ForgeError::InvalidInput(format!(
                "created PR head SHA '{delivered_commit}' does not match requested commit '{commit}'"
            )));
        }
        Ok(json!({
            "handle": self.pull_handle(number, version),
            "work_unit": work_unit,
            "commit": delivered_commit,
            "version": version
        }))
    }

    fn reflect_disposition(&self, input: Value) -> Result<Value, ForgeError> {
        let work_unit = input
            .get("work_unit")
            .ok_or_else(|| ForgeError::InvalidInput("work_unit is required".into()))?;
        self.issue_number(Some(work_unit))?;
        let pull = self.pull_number(input.get("change"))?;
        let body = required_string(&input, "body")?;
        let response = self.send(
            "POST",
            format!(
                "/repos/{}/{}/issues/{pull}/comments",
                self.config.owner, self.config.repo
            ),
            Some(json!({ "body": body })),
        )?;
        Ok(json!({
            "work_unit": work_unit,
            "change": input.get("change").cloned().unwrap_or(Value::Null),
            "receipt": receipt(response, "disposition")
        }))
    }

    fn apply_approved_change(&self, input: Value) -> Result<Value, ForgeError> {
        let work_unit = input
            .get("work_unit")
            .ok_or_else(|| ForgeError::InvalidInput("work_unit is required".into()))?;
        self.issue_number(Some(work_unit))?;
        let (pull, change_version) = self.pull_change(input.get("change"))?;
        let approved_version = required_u64(&input, "approved_version")?;
        let approved_commit = required_string(&input, "approved_commit")?;
        let base = required_string(&input, "base")?;
        if change_version != approved_version {
            return Err(ForgeError::InvalidInput(format!(
                "change version {change_version} does not match approved_version {approved_version}"
            )));
        }
        let pull_snapshot = self.send(
            "GET",
            format!(
                "/repos/{}/{}/pulls/{pull}",
                self.config.owner, self.config.repo
            ),
            None,
        )?;
        let actual_base = response_string(&pull_snapshot, &["/base/ref"])?;
        if actual_base != base {
            return Err(ForgeError::InvalidInput(format!(
                "change base '{actual_base}' does not match requested base '{base}'"
            )));
        }
        let actual_head = response_string(&pull_snapshot, &["/head/sha"])?;
        if actual_head != approved_commit {
            return Err(ForgeError::InvalidInput(format!(
                "change head SHA '{actual_head}' does not match approved commit '{approved_commit}'"
            )));
        }
        let response = self.send(
            "PUT",
            format!(
                "/repos/{}/{}/pulls/{pull}/merge",
                self.config.owner, self.config.repo
            ),
            Some(json!({ "sha": approved_commit })),
        )?;
        let applied_commit = response_string(&response, &["/sha", "/merge_commit_sha"])?;
        Ok(json!({
            "work_unit": work_unit,
            "change": input.get("change").cloned().unwrap_or(Value::Null),
            "applied_commit": applied_commit,
            "receipt": receipt(response, "applied")
        }))
    }

    fn close_out(&self, input: Value) -> Result<Value, ForgeError> {
        let work_unit = input
            .get("work_unit")
            .ok_or_else(|| ForgeError::InvalidInput("work_unit is required".into()))?;
        let number = self.issue_number(Some(work_unit))?;
        let body = required_string(&input, "body")?;
        self.send(
            "POST",
            format!(
                "/repos/{}/{}/issues/{number}/comments",
                self.config.owner, self.config.repo
            ),
            Some(json!({ "body": body })),
        )?;
        let response = self.send(
            "PATCH",
            format!(
                "/repos/{}/{}/issues/{number}",
                self.config.owner, self.config.repo
            ),
            Some(json!({ "state": "closed" })),
        )?;
        Ok(json!({ "handle": self.issue_handle(number), "receipt": receipt(response, "closed") }))
    }

    fn ticket_snapshot(&self, number: u64, response: &Value) -> Result<Value, ForgeError> {
        let number = if number == 0 {
            response
                .get("number")
                .and_then(Value::as_u64)
                .ok_or_else(|| ForgeError::ProviderResponse("missing number".into()))?
        } else {
            number
        };
        Ok(json!({
            "handle": self.issue_handle(number),
            "title": response.get("title").and_then(Value::as_str).unwrap_or(""),
            "body": response.get("body").cloned().unwrap_or(Value::Null),
            "state": response.get("state").and_then(Value::as_str).unwrap_or("unknown")
        }))
    }

    fn resolve_reference(&self, reference: &str) -> Result<u64, ForgeError> {
        let reference = reference.trim();
        if let Some(rest) = reference.strip_prefix("github:") {
            return self.resolve_coordinate_reference(rest);
        }
        if let Some((repo, number)) = reference.split_once('#') {
            if repo.is_empty() {
                return parse_number(number);
            }
            return self.resolve_coordinate_reference(reference);
        }
        parse_number(reference.strip_prefix('#').unwrap_or(reference))
    }

    fn resolve_coordinate_reference(&self, reference: &str) -> Result<u64, ForgeError> {
        let (repo, number) = reference.split_once('#').ok_or_else(|| {
            ForgeError::InvalidInput(format!("invalid GitHub reference {reference}"))
        })?;
        if repo != self.repo_scope() {
            return Err(ForgeError::ForeignScope(format!(
                "{repo} does not match {}",
                self.repo_scope()
            )));
        }
        parse_number(number)
    }

    fn issue_number(&self, value: Option<&Value>) -> Result<u64, ForgeError> {
        let handle = handle_id(value)?;
        let prefix = format!("github:{}:issue:", self.repo_scope());
        if let Some(number) = handle.strip_prefix(&prefix) {
            return parse_number(number);
        }
        if handle.starts_with("github:") {
            return Err(ForgeError::ForeignScope(format!(
                "{handle} does not match {}",
                self.repo_scope()
            )));
        }
        Err(ForgeError::InvalidInput(format!(
            "handle '{handle}' is not a GitHub issue handle"
        )))
    }

    fn pull_number(&self, value: Option<&Value>) -> Result<u64, ForgeError> {
        Ok(self.pull_change(value)?.0)
    }

    fn pull_change(&self, value: Option<&Value>) -> Result<(u64, u64), ForgeError> {
        let handle = handle_id(value)?;
        let prefix = format!("github:{}:pull:", self.repo_scope());
        if let Some(rest) = handle.strip_prefix(&prefix) {
            let mut parts = rest.split(':');
            let number = parts.next().unwrap_or_default();
            let version_label = parts.next();
            let version = parts.next();
            if version_label != Some("version") || parts.next().is_some() {
                return Err(ForgeError::InvalidInput(format!(
                    "handle '{handle}' is not a versioned GitHub pull handle"
                )));
            }
            let version = version.ok_or_else(|| {
                ForgeError::InvalidInput(format!(
                    "handle '{handle}' is not a versioned GitHub pull handle"
                ))
            })?;
            return Ok((parse_number(number)?, parse_number(version)?));
        }
        if handle.starts_with("github:") {
            return Err(ForgeError::ForeignScope(format!(
                "{handle} does not match {}",
                self.repo_scope()
            )));
        }
        Err(ForgeError::InvalidInput(format!(
            "handle '{handle}' is not a GitHub pull handle"
        )))
    }
}

fn required_string<'a>(input: &'a Value, field: &str) -> Result<&'a str, ForgeError> {
    input
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ForgeError::InvalidInput(format!("{field} is required")))
}

fn required_u64(input: &Value, field: &str) -> Result<u64, ForgeError> {
    input
        .get(field)
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or_else(|| ForgeError::InvalidInput(format!("{field} is required")))
}

fn handle_id(value: Option<&Value>) -> Result<&str, ForgeError> {
    value
        .and_then(|value| value.get("id"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ForgeError::InvalidInput("handle.id is required".into()))
}

fn parse_number(value: &str) -> Result<u64, ForgeError> {
    value
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|number| *number > 0)
        .ok_or_else(|| ForgeError::InvalidInput(format!("invalid ticket number '{value}'")))
}

fn response_string<'a>(response: &'a Value, pointers: &[&str]) -> Result<&'a str, ForgeError> {
    pointers
        .iter()
        .find_map(|pointer| response.pointer(pointer).and_then(Value::as_str))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ForgeError::ProviderResponse(format!(
                "missing provider response value at {}",
                pointers.join(" or ")
            ))
        })
}

fn receipt(response: Value, fallback: &str) -> String {
    response
        .get("html_url")
        .or_else(|| response.get("url"))
        .or_else(|| response.get("sha"))
        .and_then(Value::as_str)
        .unwrap_or(fallback)
        .to_string()
}
