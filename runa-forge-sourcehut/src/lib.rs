use runa_forge_contract::{ForgeConnector, ForgeError, Handle, Operation};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::process::Command;
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
        if request.kind == "GIT" {
            return execute_git(config, request);
        }
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
        if !status.is_success() {
            return Err(ForgeError::Transport(format!(
                "SourceHut returned {status}"
            )));
        }
        let value = response
            .json::<Value>()
            .map_err(|error| ForgeError::ProviderResponse(error.to_string()))?;
        reject_provider_error_payload(&value)?;
        Ok(value)
    }
}

#[derive(Debug, Clone)]
pub struct SourcehutConnector<T> {
    config: SourcehutConfig,
    transport: T,
    tracker_rid: Arc<Mutex<Option<String>>>,
}

impl<T: SourcehutTransport> SourcehutConnector<T> {
    pub fn new(config: SourcehutConfig, transport: T) -> Self {
        Self {
            config,
            transport,
            tracker_rid: Arc::new(Mutex::new(None)),
        }
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
        let response = self.transport.send(
            &self.config,
            ProviderRequest {
                kind: "GRAPHQL".to_string(),
                operation: operation.to_string(),
                path: "/query".to_string(),
                body: Some(json!({ "query": query, "variables": variables })),
            },
        )?;
        reject_provider_error_payload(&response)?;
        Ok(response)
    }

    fn git(&self, operation: &str, body: Value) -> Result<Value, ForgeError> {
        let response = self.transport.send(
            &self.config,
            ProviderRequest {
                kind: "GIT".to_string(),
                operation: operation.to_string(),
                path: self.config.git_remote.clone(),
                body: Some(body),
            },
        )?;
        reject_provider_error_payload(&response)?;
        Ok(response)
    }

    fn tracker_rid(&self) -> Result<String, ForgeError> {
        if let Some(rid) = self.tracker_rid_cache()?.clone() {
            return Ok(rid);
        }

        let resolved = self.resolve_tracker_rid()?;
        let mut cache = self.tracker_rid_cache()?;
        if let Some(rid) = cache.as_ref() {
            return Ok(rid.clone());
        }
        *cache = Some(resolved.clone());
        Ok(resolved)
    }

    fn tracker_rid_cache(&self) -> Result<std::sync::MutexGuard<'_, Option<String>>, ForgeError> {
        self.tracker_rid.lock().map_err(|_| {
            ForgeError::Transport("sourcehut tracker rid cache lock poisoned".to_string())
        })
    }

    fn resolve_tracker_rid(&self) -> Result<String, ForgeError> {
        let tracker_id = self.tracker_id_number()?;
        let mut cursor = Value::Null;
        loop {
            // SourceHut's trackers(cursor:) lists trackers owned by the credential
            // user. The runa deployment credential owns its configured tracker;
            // grant-only trackers need owner+name resolution outside this fix.
            let response = self.graphql(
                "trackers",
                "query trackers($cursor: Cursor) { trackers(cursor: $cursor) { results { id rid name } cursor } }",
                json!({ "cursor": cursor }),
            )?;
            let trackers = required_provider_object(&response, "/data/trackers", "trackers")?;
            let results = trackers
                .get("results")
                .and_then(Value::as_array)
                .ok_or_else(|| ForgeError::ProviderResponse("missing trackers results".into()))?;
            for tracker in results {
                if tracker.get("id").and_then(Value::as_u64) == Some(tracker_id) {
                    return tracker
                        .get("rid")
                        .and_then(Value::as_str)
                        .filter(|rid| !rid.is_empty())
                        .map(str::to_string)
                        .ok_or_else(|| {
                            ForgeError::ProviderResponse(format!(
                                "SourceHut tracker {} did not include an opaque rid",
                                self.config.tracker_id
                            ))
                        });
                }
            }

            match trackers.get("cursor") {
                None | Some(Value::Null) => break,
                Some(Value::String(next)) if next.is_empty() => break,
                Some(Value::String(next)) => cursor = json!(next),
                Some(_) => {
                    return Err(ForgeError::ProviderResponse(
                        "SourceHut trackers cursor was not a string or null".into(),
                    ));
                }
            }
        }

        Err(ForgeError::ProviderResponse(format!(
            "SourceHut tracker_id {} was not returned by trackers(cursor:)",
            self.config.tracker_id
        )))
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
        let tracker_rid = self.tracker_rid()?;
        let response = self.graphql(
            "ticket",
            "query ticket($tracker: ID!, $id: Int!) { tracker(rid: $tracker) { id rid name ticket(id: $id) { id subject body status } } }",
            json!({ "id": number, "tracker": tracker_rid }),
        )?;
        self.ticket_snapshot(
            number,
            required_provider_object(&response, "/data/tracker/ticket", "ticket")?,
        )
    }

    fn create_ticket(&self, input: Value) -> Result<Value, ForgeError> {
        let title = required_string(&input, "title")?;
        let body = required_string(&input, "body")?;
        let tracker_id = self.tracker_id_number()?;
        let response = self.graphql(
            "submitTicket",
            "mutation submitTicket($trackerId: Int!, $input: SubmitTicketInput!) { submitTicket(trackerId: $trackerId, input: $input) { id subject body status } }",
            json!({ "trackerId": tracker_id, "input": { "subject": title, "body": body } }),
        )?;
        let ticket = required_provider_object(&response, "/data/submitTicket", "submitTicket")?;
        let number = ticket.get("id").and_then(Value::as_u64).unwrap_or(0);
        self.ticket_snapshot(number, ticket)
    }

    fn claim_work_unit(&self, input: Value) -> Result<Value, ForgeError> {
        let number = self.ticket_number(input.get("handle"))?;
        let tracker_id = self.tracker_id_number()?;
        let response = self.graphql(
            "updateTicketStatus",
            "mutation claimWorkUnit($trackerId: Int!, $ticketId: Int!, $input: UpdateStatusInput!) { updateTicketStatus(trackerId: $trackerId, ticketId: $ticketId, input: $input) { id } }",
            json!({ "trackerId": tracker_id, "ticketId": number, "input": { "status": "IN_PROGRESS" } }),
        )?;
        let result =
            required_provider_object(&response, "/data/updateTicketStatus", "updateTicketStatus")?;
        Ok(json!({ "handle": self.ticket_handle(number), "receipt": receipt(result, "claimed") }))
    }

    fn record_progress(&self, input: Value) -> Result<Value, ForgeError> {
        let number = self.ticket_number(input.get("handle"))?;
        let body = required_string(&input, "body")?;
        let tracker_id = self.tracker_id_number()?;
        let response = self.graphql(
            "submitComment",
            "mutation submitComment($trackerId: Int!, $ticketId: Int!, $input: SubmitCommentInput!) { submitComment(trackerId: $trackerId, ticketId: $ticketId, input: $input) { id } }",
            json!({ "trackerId": tracker_id, "ticketId": number, "input": { "text": body } }),
        )?;
        let result = required_provider_object(&response, "/data/submitComment", "submitComment")?;
        Ok(json!({ "handle": self.ticket_handle(number), "receipt": receipt(result, "progress") }))
    }

    fn deliver_change_proposal(&self, input: Value) -> Result<Value, ForgeError> {
        let work_unit = input
            .get("work_unit")
            .ok_or_else(|| ForgeError::InvalidInput("work_unit is required".into()))?;
        self.ticket_number(Some(work_unit))?;
        let branch = required_string(&input, "branch")?;
        let commit = required_string(&input, "commit")?;
        let version = required_u64(&input, "version")?;
        let destination_ref = git_ref(branch);
        let response = self.git(
            "deliverChangeProposal",
            json!({
                "source": commit,
                "destination": destination_ref,
            }),
        )?;
        let _produced_ref = git_result_ref(&response)?;
        let produced_commit = git_result_commit(&response)?;
        Ok(json!({
            "handle": self.change_handle(branch, version),
            "work_unit": work_unit,
            "commit": produced_commit,
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
        let tracker_id = self.tracker_id_number()?;
        let response = self.graphql(
            "submitComment",
            "mutation submitComment($trackerId: Int!, $ticketId: Int!, $input: SubmitCommentInput!) { submitComment(trackerId: $trackerId, ticketId: $ticketId, input: $input) { id } }",
            json!({ "trackerId": tracker_id, "ticketId": number, "input": { "text": body } }),
        )?;
        let result = required_provider_object(&response, "/data/submitComment", "submitComment")?;
        Ok(json!({
            "work_unit": work_unit,
            "change": input.get("change").cloned().unwrap_or(Value::Null),
            "receipt": receipt(result, "disposition")
        }))
    }

    fn apply_approved_change(&self, input: Value) -> Result<Value, ForgeError> {
        let work_unit = input
            .get("work_unit")
            .ok_or_else(|| ForgeError::InvalidInput("work_unit is required".into()))?;
        self.ticket_number(Some(work_unit))?;
        let (change_branch, change_version) = self.change_parts(input.get("change"))?;
        let approved_version = required_u64(&input, "approved_version")?;
        let approved_commit = required_string(&input, "approved_commit")?;
        let base = required_string(&input, "base")?;
        if change_version != approved_version {
            return Err(ForgeError::InvalidInput(format!(
                "change version {change_version} does not match approved_version {approved_version}"
            )));
        }
        let change_ref = git_ref(change_branch);
        let delivered = self.git(
            "resolveChangeProposal",
            json!({
                "ref": change_ref,
            }),
        )?;
        let delivered_ref = git_result_ref(&delivered)?;
        let delivered_commit = git_result_commit(&delivered)?;
        if delivered_commit != approved_commit {
            return Err(ForgeError::InvalidInput(format!(
                "change ref {delivered_ref} points at '{delivered_commit}', not approved commit '{approved_commit}'"
            )));
        }
        let destination_ref = git_ref(base);
        let response = self.git(
            "applyApprovedChange",
            json!({
                "source": approved_commit,
                "destination": destination_ref,
            }),
        )?;
        let produced_ref = git_result_ref(&response)?;
        let produced_commit = git_result_commit(&response)?;
        Ok(json!({
            "work_unit": work_unit,
            "change": input.get("change").cloned().unwrap_or(Value::Null),
            "applied_commit": produced_commit,
            "receipt": produced_ref
        }))
    }

    fn close_out(&self, input: Value) -> Result<Value, ForgeError> {
        let work_unit = input
            .get("work_unit")
            .ok_or_else(|| ForgeError::InvalidInput("work_unit is required".into()))?;
        let number = self.ticket_number(Some(work_unit))?;
        let body = required_string(&input, "body")?;
        let tracker_id = self.tracker_id_number()?;
        let response = self.graphql(
            "submitComment",
            "mutation submitComment($trackerId: Int!, $ticketId: Int!, $input: SubmitCommentInput!) { submitComment(trackerId: $trackerId, ticketId: $ticketId, input: $input) { id } }",
            json!({
                "trackerId": tracker_id,
                "ticketId": number,
                "input": {
                    "text": body,
                    "status": "RESOLVED",
                    "resolution": "IMPLEMENTED"
                }
            }),
        )?;
        let result = required_provider_object(&response, "/data/submitComment", "submitComment")?;
        Ok(json!({ "handle": self.ticket_handle(number), "receipt": receipt(result, "closed") }))
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
            "body": ticket
                .get("body")
                .or_else(|| ticket.get("description"))
                .cloned()
                .unwrap_or(Value::Null),
            "state": ticket.get("status").and_then(Value::as_str).unwrap_or("unknown")
        }))
    }

    fn tracker_id_number(&self) -> Result<u64, ForgeError> {
        parse_number(&self.config.tracker_id).map_err(|_| {
            ForgeError::InvalidInput(format!(
                "sourcehut tracker_id '{}' must be a positive integer",
                self.config.tracker_id
            ))
        })
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
        let _ = self.change_parts(value)?;
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

    fn change_parts<'a>(&self, value: Option<&'a Value>) -> Result<(&'a str, u64), ForgeError> {
        let handle = handle_id(value)?;
        let prefix = format!("sourcehut:tracker:{}:change:", self.config.tracker_id);
        if let Some(change) = handle.strip_prefix(&prefix) {
            let (branch, version) = change.rsplit_once(":version:").ok_or_else(|| {
                ForgeError::InvalidInput(format!(
                    "handle '{handle}' is not a versioned SourceHut change handle"
                ))
            })?;
            if branch.is_empty() {
                return Err(ForgeError::InvalidInput(format!(
                    "handle '{handle}' is not a versioned SourceHut change handle"
                )));
            }
            return Ok((branch, parse_number(version)?));
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

fn git_ref(value: &str) -> String {
    if value.starts_with("refs/") {
        value.to_string()
    } else {
        format!("refs/heads/{value}")
    }
}

fn git_result_ref(value: &Value) -> Result<&str, ForgeError> {
    value
        .get("ref")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ForgeError::ProviderResponse("missing git result ref".into()))
}

fn git_result_commit(value: &Value) -> Result<&str, ForgeError> {
    value
        .get("commit")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ForgeError::ProviderResponse("missing git result commit".into()))
}

fn reject_provider_error_payload(value: &Value) -> Result<(), ForgeError> {
    let Some(errors) = value.get("errors").and_then(Value::as_array) else {
        return Ok(());
    };
    Err(ForgeError::ProviderResponse(format!(
        "SourceHut GraphQL provider response contained top-level errors array with {} entr{}",
        errors.len(),
        if errors.len() == 1 { "y" } else { "ies" }
    )))
}

fn required_provider_object<'a>(
    value: &'a Value,
    pointer: &str,
    name: &str,
) -> Result<&'a Value, ForgeError> {
    value
        .pointer(pointer)
        .filter(|candidate| candidate.is_object())
        .ok_or_else(|| ForgeError::ProviderResponse(format!("missing {name} result")))
}

fn execute_git(config: &SourcehutConfig, request: ProviderRequest) -> Result<Value, ForgeError> {
    let remote = config.git_remote.trim();
    if remote.is_empty() {
        return Err(ForgeError::InvalidInput(
            "sourcehut git_remote is required".into(),
        ));
    }
    let body = request
        .body
        .ok_or_else(|| ForgeError::InvalidInput("git request body is required".into()))?;
    if request.operation == "resolveChangeProposal" {
        let change_ref = body
            .get("ref")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ForgeError::InvalidInput("git ref is required".into()))?;
        return resolve_git_ref(remote, change_ref);
    }
    let source = body
        .get("source")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ForgeError::InvalidInput("git source is required".into()))?;
    let destination = body
        .get("destination")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ForgeError::InvalidInput("git destination is required".into()))?;

    let push = Command::new("git")
        .args([
            "push",
            "--porcelain",
            remote,
            &format!("{source}:{destination}"),
        ])
        .output()
        .map_err(|error| ForgeError::Transport(format!("git push failed: {error}")))?;
    if !push.status.success() {
        return Err(ForgeError::Transport(format!(
            "git push exited with status {}\n{}",
            push.status,
            String::from_utf8_lossy(&push.stderr)
        )));
    }

    let resolved = resolve_git_ref(remote, destination)?;

    Ok(json!({
        "commit": git_result_commit(&resolved)?,
        "ref": git_result_ref(&resolved)?,
        "push": String::from_utf8_lossy(&push.stdout).to_string()
    }))
}

fn resolve_git_ref(remote: &str, target_ref: &str) -> Result<Value, ForgeError> {
    let ls_remote = Command::new("git")
        .args(["ls-remote", remote, target_ref])
        .output()
        .map_err(|error| ForgeError::Transport(format!("git ls-remote failed: {error}")))?;
    if !ls_remote.status.success() {
        return Err(ForgeError::Transport(format!(
            "git ls-remote exited with status {}\n{}",
            ls_remote.status,
            String::from_utf8_lossy(&ls_remote.stderr)
        )));
    }
    let stdout = String::from_utf8(ls_remote.stdout).map_err(|_| {
        ForgeError::ProviderResponse("git ls-remote produced non-UTF-8 output".into())
    })?;
    let Some((commit, produced_ref)) = stdout
        .lines()
        .find_map(|line| line.split_once('\t'))
        .filter(|(_, produced_ref)| *produced_ref == target_ref)
    else {
        return Err(ForgeError::ProviderResponse(format!(
            "git remote did not contain ref {target_ref}"
        )));
    };

    Ok(json!({
        "commit": commit,
        "ref": produced_ref
    }))
}

fn receipt(response: &Value, fallback: &str) -> String {
    response
        .pointer("/data")
        .and_then(first_id)
        .or_else(|| first_id(response))
        .unwrap_or_else(|| fallback.to_string())
}

fn first_id(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            if let Some(id) = map.get("id") {
                if let Some(id) = id.as_str().filter(|id| !id.is_empty()) {
                    return Some(id.to_string());
                }
                if let Some(id) = id.as_u64() {
                    return Some(id.to_string());
                }
            }
            map.values().find_map(first_id)
        }
        Value::Array(values) => values.iter().find_map(first_id),
        _ => None,
    }
}
