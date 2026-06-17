use runa_forge_contract::{ForgeConnector, ForgeError, Handle, Operation};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcehutConfig {
    pub tracker_id: String,
    pub api_base: String,
    pub git_remote: String,
    pub credential_env: Option<String>,
    pub credential_command: Option<Vec<String>>,
}

impl SourcehutConfig {
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
    pub kind: String,
    pub operation: String,
    pub path: String,
    pub body: Option<Value>,
}

pub trait SourcehutTransport: Clone + Send + Sync + 'static {
    fn send(&self, config: &SourcehutConfig, request: ProviderRequest)
    -> Result<Value, ForgeError>;
}

#[derive(Debug, Clone, Default)]
pub struct SourcehutRecordingTransport {
    requests: Arc<Mutex<Vec<ProviderRequest>>>,
    responses: Arc<Mutex<VecDeque<Value>>>,
    repeating: Arc<Mutex<Option<Value>>>,
}

impl SourcehutRecordingTransport {
    pub fn with_repeating_response(response: Value) -> Self {
        let transport = Self::default();
        *transport.repeating.lock().unwrap() = Some(response);
        transport
    }

    pub fn requests(&self) -> Vec<ProviderRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl SourcehutTransport for SourcehutRecordingTransport {
    fn send(
        &self,
        _config: &SourcehutConfig,
        request: ProviderRequest,
    ) -> Result<Value, ForgeError> {
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
pub struct SourcehutHttpTransport;

impl SourcehutTransport for SourcehutHttpTransport {
    fn send(
        &self,
        config: &SourcehutConfig,
        request: ProviderRequest,
    ) -> Result<Value, ForgeError> {
        if request.kind != "GRAPHQL" {
            return Err(ForgeError::Unsupported(format!(
                "production HTTP transport cannot execute {}",
                request.kind
            )));
        }
        let client = reqwest::blocking::Client::new();
        let mut builder = client.post(&config.api_base);
        if let Some(token) = config.credential()? {
            builder = builder.bearer_auth(token);
        }
        let response = builder
            .json(&request.body.unwrap_or_else(|| json!({})))
            .send()
            .map_err(|error| ForgeError::Transport(error.to_string()))?;
        let status = response.status();
        let value = response
            .json::<Value>()
            .map_err(|error| ForgeError::ProviderResponse(error.to_string()))?;
        if !status.is_success() {
            return Err(ForgeError::Transport(format!(
                "SourceHut returned {status}"
            )));
        }
        Ok(value)
    }
}

#[derive(Debug, Clone)]
pub struct SourcehutConnector<T> {
    config: SourcehutConfig,
    transport: T,
}

impl<T: SourcehutTransport> SourcehutConnector<T> {
    pub fn new(config: SourcehutConfig, transport: T) -> Self {
        Self { config, transport }
    }

    pub fn call(&self, operation: Operation, input: Value) -> Result<Value, ForgeError> {
        <Self as ForgeConnector>::call(self, operation, input)
    }

    fn ticket_handle(&self, number: u64) -> Handle {
        Handle {
            id: format!(
                "sourcehut:tracker:{}:ticket:{number}",
                self.config.tracker_id
            ),
            display: format!("sourcehut:{}#{number}", self.config.tracker_id),
        }
    }

    fn change_handle(&self, branch: &str, version: u64) -> Handle {
        Handle {
            id: format!(
                "sourcehut:tracker:{}:change:{branch}:version:{version}",
                self.config.tracker_id
            ),
            display: branch.to_string(),
        }
    }

    fn graphql(&self, operation: &str, query: &str, variables: Value) -> Result<Value, ForgeError> {
        self.transport.send(
            &self.config,
            ProviderRequest {
                kind: "GRAPHQL".to_string(),
                operation: operation.to_string(),
                path: "/query".to_string(),
                body: Some(json!({ "query": query, "variables": variables })),
            },
        )
    }

    fn git(&self, operation: &str, body: Value) -> Result<Value, ForgeError> {
        self.transport.send(
            &self.config,
            ProviderRequest {
                kind: "GIT".to_string(),
                operation: operation.to_string(),
                path: self.config.git_remote.clone(),
                body: Some(body),
            },
        )
    }
}

impl<T: SourcehutTransport> ForgeConnector for SourcehutConnector<T> {
    fn set_name(&self) -> &str {
        "sourcehut"
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

impl<T: SourcehutTransport> SourcehutConnector<T> {
    fn read_ticket(&self, input: Value) -> Result<Value, ForgeError> {
        let reference = required_string(&input, "reference")?;
        let number = self.resolve_reference(reference)?;
        let response = self.graphql(
            "ticket",
            "query ticket($id: Int!) { ticket(id: $id) { id subject description status } }",
            json!({ "id": number, "tracker": self.config.tracker_id }),
        )?;
        self.ticket_snapshot(
            number,
            response.pointer("/data/ticket").unwrap_or(&response),
        )
    }

    fn create_ticket(&self, input: Value) -> Result<Value, ForgeError> {
        let title = required_string(&input, "title")?;
        let body = required_string(&input, "body")?;
        let response = self.graphql(
            "createTicket",
            "mutation createTicket($subject: String!, $description: String!) { createTicket(subject: $subject, description: $description) { id subject description status } }",
            json!({ "subject": title, "description": body, "tracker": self.config.tracker_id }),
        )?;
        let ticket = response
            .pointer("/data/createTicket")
            .or_else(|| response.pointer("/data/ticket"))
            .unwrap_or(&response);
        let number = ticket.get("id").and_then(Value::as_u64).unwrap_or(0);
        self.ticket_snapshot(number, ticket)
    }

    fn claim_work_unit(&self, input: Value) -> Result<Value, ForgeError> {
        let number = self.ticket_number(input.get("handle"))?;
        let response = self.graphql(
            "claimWorkUnit",
            "mutation claimWorkUnit($id: Int!) { updateTicket(id: $id) { id } }",
            json!({ "id": number }),
        )?;
        Ok(json!({ "handle": self.ticket_handle(number), "receipt": receipt(response, "claimed") }))
    }

    fn record_progress(&self, input: Value) -> Result<Value, ForgeError> {
        let number = self.ticket_number(input.get("handle"))?;
        let body = required_string(&input, "body")?;
        let response = self.graphql(
            "recordProgress",
            "mutation recordProgress($id: Int!, $body: String!) { createComment(id: $id, body: $body) { id } }",
            json!({ "id": number, "body": body }),
        )?;
        Ok(
            json!({ "handle": self.ticket_handle(number), "receipt": receipt(response, "progress") }),
        )
    }

    fn deliver_change_proposal(&self, input: Value) -> Result<Value, ForgeError> {
        let work_unit = input
            .get("work_unit")
            .ok_or_else(|| ForgeError::InvalidInput("work_unit is required".into()))?;
        self.ticket_number(Some(work_unit))?;
        let branch = required_string(&input, "branch")?;
        let commit = required_string(&input, "commit")?;
        let version = required_u64(&input, "version")?;
        let _ = self.git(
            "deliverChangeProposal",
            json!({ "remote": self.config.git_remote, "branch": branch, "commit": commit }),
        )?;
        Ok(json!({
            "handle": self.change_handle(branch, version),
            "work_unit": work_unit,
            "commit": commit,
            "version": version
        }))
    }

    fn reflect_disposition(&self, input: Value) -> Result<Value, ForgeError> {
        let work_unit = input
            .get("work_unit")
            .ok_or_else(|| ForgeError::InvalidInput("work_unit is required".into()))?;
        let number = self.ticket_number(Some(work_unit))?;
        self.change_id(input.get("change"))?;
        let body = required_string(&input, "body")?;
        let response = self.graphql(
            "reflectDisposition",
            "mutation reflectDisposition($id: Int!, $body: String!) { createComment(id: $id, body: $body) { id } }",
            json!({ "id": number, "body": body }),
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
        self.ticket_number(Some(work_unit))?;
        let change = self.change_id(input.get("change"))?;
        let approved_commit = required_string(&input, "approved_commit")?;
        let _ = self.git(
            "applyApprovedChange",
            json!({ "remote": self.config.git_remote, "change": change, "commit": approved_commit }),
        )?;
        Ok(json!({
            "work_unit": work_unit,
            "change": input.get("change").cloned().unwrap_or(Value::Null),
            "applied_commit": approved_commit,
            "receipt": "applied"
        }))
    }

    fn close_out(&self, input: Value) -> Result<Value, ForgeError> {
        let work_unit = input
            .get("work_unit")
            .ok_or_else(|| ForgeError::InvalidInput("work_unit is required".into()))?;
        let number = self.ticket_number(Some(work_unit))?;
        let body = required_string(&input, "body")?;
        let response = self.graphql(
            "closeOut",
            "mutation closeOut($id: Int!, $body: String!) { closeTicket(id: $id, body: $body) { id } }",
            json!({ "id": number, "body": body }),
        )?;
        Ok(json!({ "handle": self.ticket_handle(number), "receipt": receipt(response, "closed") }))
    }

    fn ticket_snapshot(&self, number: u64, ticket: &Value) -> Result<Value, ForgeError> {
        let number = if number == 0 {
            ticket
                .get("id")
                .and_then(Value::as_u64)
                .ok_or_else(|| ForgeError::ProviderResponse("missing ticket id".into()))?
        } else {
            number
        };
        Ok(json!({
            "handle": self.ticket_handle(number),
            "title": ticket.get("subject").and_then(Value::as_str).unwrap_or(""),
            "body": ticket.get("description").cloned().unwrap_or(Value::Null),
            "state": ticket.get("status").and_then(Value::as_str).unwrap_or("unknown")
        }))
    }

    fn resolve_reference(&self, reference: &str) -> Result<u64, ForgeError> {
        let reference = reference.trim();
        if let Some(rest) = reference.strip_prefix("sourcehut:") {
            let (tracker, number) = rest.split_once('#').ok_or_else(|| {
                ForgeError::InvalidInput(format!("invalid SourceHut reference {reference}"))
            })?;
            if tracker != self.config.tracker_id {
                return Err(ForgeError::ForeignScope(format!(
                    "{tracker} does not match {}",
                    self.config.tracker_id
                )));
            }
            return parse_number(number);
        }
        parse_number(reference.strip_prefix('#').unwrap_or(reference))
    }

    fn ticket_number(&self, value: Option<&Value>) -> Result<u64, ForgeError> {
        let handle = handle_id(value)?;
        let prefix = format!("sourcehut:tracker:{}:ticket:", self.config.tracker_id);
        if let Some(number) = handle.strip_prefix(&prefix) {
            return parse_number(number);
        }
        if handle.starts_with("sourcehut:tracker:") {
            return Err(ForgeError::ForeignScope(format!(
                "{handle} does not match tracker {}",
                self.config.tracker_id
            )));
        }
        Err(ForgeError::InvalidInput(format!(
            "handle '{handle}' is not a SourceHut ticket handle"
        )))
    }

    fn change_id<'a>(&self, value: Option<&'a Value>) -> Result<&'a str, ForgeError> {
        let handle = handle_id(value)?;
        let prefix = format!("sourcehut:tracker:{}:change:", self.config.tracker_id);
        if let Some(change) = handle.strip_prefix(&prefix) {
            return Ok(change);
        }
        if handle.starts_with("sourcehut:tracker:") {
            return Err(ForgeError::ForeignScope(format!(
                "{handle} does not match tracker {}",
                self.config.tracker_id
            )));
        }
        Err(ForgeError::InvalidInput(format!(
            "handle '{handle}' is not a SourceHut change handle"
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

fn receipt(response: Value, fallback: &str) -> String {
    response
        .pointer("/data/id")
        .or_else(|| response.get("id"))
        .and_then(Value::as_u64)
        .map(|id| id.to_string())
        .unwrap_or_else(|| fallback.to_string())
}
