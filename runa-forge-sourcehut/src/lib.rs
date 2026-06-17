use std::fmt;
use std::sync::{Arc, Mutex};

use runa_forge_contract::{ForgeOperation, ForgeToolSet, Handle, forge_tool_set};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceHutConfig {
    pub owner: String,
    pub repo: String,
    pub tracker_id: u64,
    pub endpoint: String,
    pub repo_id: u64,
    pub credential_env: Option<String>,
    pub credential_command: Option<Vec<String>>,
}

impl SourceHutConfig {
    pub fn new(
        owner: impl Into<String>,
        repo: impl Into<String>,
        tracker_id: u64,
        endpoint: impl Into<String>,
        repo_id: u64,
    ) -> Self {
        Self {
            owner: owner.into(),
            repo: repo.into(),
            tracker_id,
            endpoint: endpoint.into(),
            repo_id,
            credential_env: None,
            credential_command: None,
        }
    }

    pub fn todo_query_url(&self) -> String {
        format!("https://todo.{}/query", self.endpoint)
    }

    pub fn git_remote(&self) -> String {
        format!("git@git.{}:~{}/{}", self.endpoint, self.owner, self.repo)
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
pub struct RecordingSourceHutTransport {
    requests: Arc<Mutex<Vec<ProviderRequest>>>,
}

impl RecordingSourceHutTransport {
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
pub struct SourceHutConnector {
    config: SourceHutConfig,
    transport: RecordingSourceHutTransport,
}

impl SourceHutConnector {
    pub fn new_for_test(config: SourceHutConfig, transport: RecordingSourceHutTransport) -> Self {
        Self { config, transport }
    }

    pub fn config(&self) -> &SourceHutConfig {
        &self.config
    }

    pub fn tool_set(&self) -> ForgeToolSet {
        forge_tool_set("sourcehut")
    }

    pub fn resolve_reference(&self, reference: &str) -> Result<Handle, ForgeSourceHutError> {
        let ticket = self.parse_ticket_number(reference)?;
        Ok(self.work_unit_handle(ticket))
    }

    pub fn validate_work_unit_handle_id(
        &self,
        handle_id: &str,
    ) -> Result<u64, ForgeSourceHutError> {
        let prefix = format!("sourcehut:tracker:{}:ticket:", self.config.tracker_id);
        let Some(number) = handle_id.strip_prefix(&prefix) else {
            return Err(ForgeSourceHutError::OutOfScope(handle_id.to_owned()));
        };
        parse_positive_number(number)
    }

    pub fn call(
        &self,
        operation: ForgeOperation,
        input: Value,
    ) -> Result<Value, ForgeSourceHutError> {
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

    fn read_ticket(&self, input: &Value) -> Result<Value, ForgeSourceHutError> {
        let reference = input
            .get("reference")
            .and_then(Value::as_str)
            .ok_or_else(|| ForgeSourceHutError::InvalidInput("missing reference".to_owned()))?;
        let handle = self.resolve_reference(reference)?;
        let ticket = self.validate_work_unit_handle_id(&handle.id)?;
        self.graphql(
            "ticket",
            ticket,
            json!({ "trackerId": self.config.tracker_id, "ticketId": ticket }),
        );
        Ok(json!({
            "handle": handle,
            "title": "",
            "body": "",
            "state": "open",
            "url": self.ticket_url(ticket)
        }))
    }

    fn create_ticket(&self, input: &Value) -> Result<Value, ForgeSourceHutError> {
        let title = string_field(input, "title")?;
        let body = input
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let ticket = 0;
        self.graphql(
            "createTicket",
            ticket,
            json!({
                "trackerId": self.config.tracker_id,
                "title": title,
                "body": body
            }),
        );
        Ok(json!({
            "handle": self.work_unit_handle(ticket),
            "title": title,
            "body": body,
            "state": "open",
            "url": self.ticket_url(ticket)
        }))
    }

    fn claim_work_unit(&self, input: &Value) -> Result<Value, ForgeSourceHutError> {
        let (ticket, handle) = self.work_unit_from_input(input)?;
        self.graphql(
            "assignUser",
            ticket,
            json!({ "trackerId": self.config.tracker_id, "ticketId": ticket }),
        );
        Ok(effect(handle, "claimed", self.ticket_url(ticket)))
    }

    fn record_progress(&self, input: &Value) -> Result<Value, ForgeSourceHutError> {
        let (ticket, handle) = self.work_unit_from_input(input)?;
        let body = string_field(input, "body")?;
        self.graphql(
            "submitComment",
            ticket,
            json!({
                "trackerId": self.config.tracker_id,
                "ticketId": ticket,
                "body": body
            }),
        );
        Ok(effect(handle, "progress-recorded", self.ticket_url(ticket)))
    }

    fn deliver_change_proposal(&self, input: &Value) -> Result<Value, ForgeSourceHutError> {
        let (ticket, handle) = self.work_unit_from_input(input)?;
        let commit = string_field(input, "commit")?;
        let version = string_field(input, "version")?;
        let branch = string_field(input, "branch")?;
        self.record_git(
            "pushProposal",
            json!({
                "remote": self.config.git_remote(),
                "source": branch,
                "destination": format!("refs/proposals/{ticket}/{version}")
            }),
        );
        let change = Handle::new(
            format!(
                "sourcehut:tracker:{}:ticket:{ticket}:proposal:{version}",
                self.config.tracker_id
            ),
            format!(
                "sourcehut:{}#{}@{}",
                self.config.tracker_id, ticket, version
            ),
        );
        Ok(json!({
            "change": change,
            "work_unit": handle,
            "commit": commit,
            "version": version,
            "url": self.ticket_url(ticket)
        }))
    }

    fn reflect_disposition(&self, input: &Value) -> Result<Value, ForgeSourceHutError> {
        let (ticket, handle) = self.work_unit_from_input(input)?;
        let body = input
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let disposition = string_field(input, "disposition")?;
        self.graphql(
            "submitComment",
            ticket,
            json!({
                "trackerId": self.config.tracker_id,
                "ticketId": ticket,
                "body": format!("{disposition}: {body}")
            }),
        );
        Ok(effect(
            handle,
            "disposition-reflected",
            self.ticket_url(ticket),
        ))
    }

    fn apply_approved_change(&self, input: &Value) -> Result<Value, ForgeSourceHutError> {
        let (ticket, handle) = self.work_unit_from_input(input)?;
        let change = handle_field(input, "change")?;
        let commit = string_field(input, "approved_commit")?;
        self.record_git(
            "applyProposal",
            json!({
                "remote": self.config.git_remote(),
                "commit": commit
            }),
        );
        Ok(json!({
            "work_unit": handle,
            "change": change,
            "applied_commit": commit,
            "status": "applied",
            "url": self.ticket_url(ticket)
        }))
    }

    fn close_out(&self, input: &Value) -> Result<Value, ForgeSourceHutError> {
        let (ticket, handle) = self.work_unit_from_input(input)?;
        let body = input
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or_default();
        self.graphql(
            "submitComment",
            ticket,
            json!({
                "trackerId": self.config.tracker_id,
                "ticketId": ticket,
                "body": body
            }),
        );
        self.graphql(
            "closeTicket",
            ticket,
            json!({
                "trackerId": self.config.tracker_id,
                "ticketId": ticket
            }),
        );
        Ok(effect(handle, "closed-out", self.ticket_url(ticket)))
    }

    fn work_unit_from_input(&self, input: &Value) -> Result<(u64, Handle), ForgeSourceHutError> {
        let handle = input
            .get("work_unit")
            .or_else(|| input.get("handle"))
            .ok_or_else(|| ForgeSourceHutError::InvalidInput("missing work_unit handle".to_owned()))
            .and_then(parse_handle)?;
        let ticket = self.validate_work_unit_handle_id(&handle.id)?;
        Ok((ticket, handle))
    }

    fn parse_ticket_number(&self, reference: &str) -> Result<u64, ForgeSourceHutError> {
        if let Some(rest) = reference.strip_prefix("sourcehut:") {
            let expected = format!("{}#", self.config.tracker_id);
            let Some(number) = rest.strip_prefix(&expected) else {
                return Err(ForgeSourceHutError::OutOfScope(reference.to_owned()));
            };
            return parse_positive_number(number);
        }

        if let Some(number) = reference.strip_prefix('#') {
            return parse_positive_number(number);
        }

        parse_positive_number(reference)
    }

    fn work_unit_handle(&self, ticket: u64) -> Handle {
        Handle::new(
            format!(
                "sourcehut:tracker:{}:ticket:{ticket}",
                self.config.tracker_id
            ),
            format!("sourcehut:{}#{ticket}", self.config.tracker_id),
        )
    }

    fn ticket_url(&self, ticket: u64) -> String {
        format!(
            "https://todo.{}/~{}/{}#{}",
            self.config.endpoint, self.config.owner, self.config.repo, ticket
        )
    }

    fn graphql(&self, operation: &str, ticket: u64, body: Value) {
        self.transport.record(ProviderRequest {
            summary: format!(
                "sourcehut graphql {operation} tracker={} ticket={ticket}",
                self.config.tracker_id
            ),
            method: "POST".to_owned(),
            path: self.config.todo_query_url(),
            body,
        });
    }

    fn record_git(&self, operation: &str, body: Value) {
        self.transport.record(ProviderRequest {
            summary: format!("sourcehut git {operation} repo={}", self.config.repo_id),
            method: "GIT".to_owned(),
            path: self.config.git_remote(),
            body,
        });
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForgeSourceHutError {
    InvalidInput(String),
    InvalidReference(String),
    OutOfScope(String),
}

impl fmt::Display for ForgeSourceHutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ForgeSourceHutError::InvalidInput(message) => f.write_str(message),
            ForgeSourceHutError::InvalidReference(reference) => {
                write!(f, "invalid SourceHut reference `{reference}`")
            }
            ForgeSourceHutError::OutOfScope(reference) => {
                write!(
                    f,
                    "SourceHut reference `{reference}` is outside connector scope"
                )
            }
        }
    }
}

impl std::error::Error for ForgeSourceHutError {}

fn parse_positive_number(value: &str) -> Result<u64, ForgeSourceHutError> {
    let number = value
        .parse::<u64>()
        .map_err(|_| ForgeSourceHutError::InvalidReference(value.to_owned()))?;
    if number == 0 {
        return Err(ForgeSourceHutError::InvalidReference(value.to_owned()));
    }
    Ok(number)
}

fn string_field<'a>(input: &'a Value, field: &str) -> Result<&'a str, ForgeSourceHutError> {
    input
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| ForgeSourceHutError::InvalidInput(format!("missing {field}")))
}

fn handle_field(input: &Value, field: &str) -> Result<Handle, ForgeSourceHutError> {
    input
        .get(field)
        .ok_or_else(|| ForgeSourceHutError::InvalidInput(format!("missing {field} handle")))
        .and_then(parse_handle)
}

fn parse_handle(value: &Value) -> Result<Handle, ForgeSourceHutError> {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| ForgeSourceHutError::InvalidInput("missing handle id".to_owned()))?;
    let display = value
        .get("display")
        .and_then(Value::as_str)
        .ok_or_else(|| ForgeSourceHutError::InvalidInput("missing handle display".to_owned()))?;
    Ok(Handle::new(id, display))
}

fn effect(handle: Handle, status: &str, url: String) -> Value {
    json!({
        "work_unit": handle,
        "status": status,
        "url": url
    })
}
