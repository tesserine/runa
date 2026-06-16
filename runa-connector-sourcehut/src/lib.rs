use std::process::Command;

use reqwest::blocking::Client;
use runa_forge_capability::{
    ForgeConnector, ForgeError, ForgeOperation, ForgeToolSet, Handle, canonical_tool_set,
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Clone, Deserialize)]
pub struct SourceHutConfig {
    pub endpoint: String,
    pub owner: String,
    pub name: String,
    pub tracker_id: u64,
    pub assignee_user_id: Option<u64>,
    pub credentials: Option<CredentialConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CredentialConfig {
    pub env: Option<String>,
    pub command: Option<Vec<String>>,
}

pub struct SourceHutConnector {
    config: SourceHutConfig,
    client: Client,
}

impl SourceHutConnector {
    pub fn new(config: SourceHutConfig) -> Self {
        Self {
            config,
            client: Client::new(),
        }
    }

    pub fn from_value(value: &toml::Value) -> Result<Self, ForgeError> {
        let config = value.clone().try_into().map_err(|error| {
            ForgeError::new(format!("invalid sourcehut connector config: {error}"))
        })?;
        Ok(Self::new(config))
    }

    pub fn endpoint(&self) -> &str {
        &self.config.endpoint
    }

    fn todo_graphql_url(&self) -> String {
        format!("https://todo.{}/query", self.config.endpoint)
    }

    fn ticket_handle(&self, number: u64) -> Handle {
        Handle {
            id: format!("sourcehut:ticket:{}:{number}", self.config.tracker_id),
            display: format!(
                "https://todo.{}/~{}/{}/{number}",
                self.config.endpoint, self.config.owner, self.config.name
            ),
        }
    }

    fn change_handle(&self, id: &str) -> Handle {
        Handle {
            id: format!("sourcehut:change:{id}"),
            display: id.to_string(),
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
        let expected_prefix = format!("sourcehut:ticket:{}:", self.config.tracker_id);
        handle_id(handle)?
            .strip_prefix(&expected_prefix)
            .ok_or_else(|| ForgeError::new("handle does not belong to this sourcehut tracker"))?
            .parse()
            .map_err(|_| ForgeError::new("sourcehut ticket handle has invalid number"))
    }

    fn graphql(&self, query: &str, variables: Value) -> Result<Value, ForgeError> {
        let token = resolve_token(self.config.credentials.as_ref())?;
        let response = self
            .client
            .post(self.todo_graphql_url())
            .bearer_auth(token)
            .json(&json!({ "query": query, "variables": variables }))
            .send()
            .map_err(|error| ForgeError::new(format!("sourcehut request failed: {error}")))?;
        if !response.status().is_success() {
            return Err(ForgeError::new(format!(
                "sourcehut request failed with status {}",
                response.status()
            )));
        }
        let value: Value = response.json().map_err(|error| {
            ForgeError::new(format!("sourcehut returned invalid JSON: {error}"))
        })?;
        if let Some(errors) = value.get("errors") {
            return Err(ForgeError::new(format!(
                "sourcehut GraphQL error: {errors}"
            )));
        }
        Ok(value.get("data").cloned().unwrap_or(Value::Null))
    }

    fn read_ticket(&self, reference: &str) -> Result<Value, ForgeError> {
        let number = self.ticket_number_from_reference(reference)?;
        let data = self.graphql(
            r#"
            query Ticket($trackerId: Int!, $number: Int!) {
              tracker(id: $trackerId) {
                ticket(number: $number) { id number subject body status }
              }
            }
            "#,
            json!({ "trackerId": self.config.tracker_id, "number": number }),
        )?;
        let ticket = data
            .pointer("/tracker/ticket")
            .cloned()
            .unwrap_or_else(|| json!({}));
        Ok(json!({
            "handle": self.ticket_handle(number),
            "title": ticket.get("subject").and_then(Value::as_str).unwrap_or_default(),
            "body": ticket.get("body").and_then(Value::as_str).unwrap_or_default(),
            "state": ticket.get("status").and_then(Value::as_str).unwrap_or_default(),
            "url": self.ticket_handle(number).display
        }))
    }

    fn create_ticket(&self, title: &str, body: &str) -> Result<Value, ForgeError> {
        let data = self.graphql(
            r#"
            mutation CreateTicket($trackerId: Int!, $subject: String!, $body: String!) {
              createTicket(trackerId: $trackerId, subject: $subject, body: $body) {
                ticket { number subject body status }
              }
            }
            "#,
            json!({ "trackerId": self.config.tracker_id, "subject": title, "body": body }),
        )?;
        let ticket = data
            .pointer("/createTicket/ticket")
            .ok_or_else(|| ForgeError::new("createTicket response omitted ticket"))?;
        let number = ticket
            .get("number")
            .and_then(Value::as_u64)
            .ok_or_else(|| ForgeError::new("created ticket response omitted number"))?;
        Ok(json!({
            "handle": self.ticket_handle(number),
            "title": ticket.get("subject").and_then(Value::as_str).unwrap_or(title),
            "body": ticket.get("body").and_then(Value::as_str).unwrap_or(body),
            "state": ticket.get("status").and_then(Value::as_str).unwrap_or("reported"),
            "url": self.ticket_handle(number).display
        }))
    }

    fn comment_ticket(&self, number: u64, body: &str) -> Result<String, ForgeError> {
        let _ = self.graphql(
            r#"
            mutation CommentTicket($trackerId: Int!, $number: Int!, $body: String!) {
              createTicketComment(trackerId: $trackerId, number: $number, body: $body) {
                comment { id }
              }
            }
            "#,
            json!({ "trackerId": self.config.tracker_id, "number": number, "body": body }),
        )?;
        Ok(format!("sourcehut ticket {number} commented"))
    }
}

impl ForgeConnector for SourceHutConnector {
    fn provider(&self) -> &'static str {
        "sourcehut"
    }

    fn tool_set(&self) -> ForgeToolSet {
        canonical_tool_set("forge:sourcehut")
    }

    fn call(&self, operation: ForgeOperation, input: Value) -> Result<Value, ForgeError> {
        match operation {
            ForgeOperation::ReadTicket => self.read_ticket(required_str(&input, "reference")?),
            ForgeOperation::CreateTicket => self.create_ticket(
                required_str(&input, "title")?,
                required_str(&input, "body")?,
            ),
            ForgeOperation::ClaimWorkUnit => {
                let handle_value = input.get("handle").unwrap_or(&Value::Null);
                let number = self.ticket_number_from_handle(handle_value)?;
                if let Some(user_id) = self.config.assignee_user_id {
                    let _ = self.graphql(
                        r#"
                        mutation AssignTicket($trackerId: Int!, $number: Int!, $userId: Int!) {
                          assignTicket(trackerId: $trackerId, number: $number, userId: $userId) { ticket { number } }
                        }
                        "#,
                        json!({ "trackerId": self.config.tracker_id, "number": number, "userId": user_id }),
                    )?;
                }
                let receipt = self.comment_ticket(number, "Claimed by runa.")?;
                Ok(json!({"handle": self.ticket_handle(number), "receipt": receipt}))
            }
            ForgeOperation::RecordProgress => {
                let handle_value = input.get("handle").unwrap_or(&Value::Null);
                let number = self.ticket_number_from_handle(handle_value)?;
                let receipt = self.comment_ticket(number, required_str(&input, "body")?)?;
                Ok(json!({"handle": self.ticket_handle(number), "receipt": receipt}))
            }
            ForgeOperation::DeliverChangeProposal => {
                let work_unit = input.get("work_unit").cloned().unwrap_or(Value::Null);
                let branch = required_str(&input, "branch")?;
                let commit = required_str(&input, "commit")?;
                let base = required_str(&input, "base")?;
                let summary = required_str(&input, "summary")?;
                let body = required_str(&input, "body")?;
                let version = input.get("version").and_then(Value::as_u64).unwrap_or(1);
                let proposal_ref = format!("{branch}@{commit}:v{version}");
                let number = self.ticket_number_from_handle(&work_unit)?;
                let receipt = self.comment_ticket(
                    number,
                    &format!("Change proposal v{version}: {summary}\n\nbase: {base}\ncommit: {commit}\nbranch: {branch}\n\n{body}"),
                )?;
                Ok(json!({
                    "work_unit": work_unit,
                    "change": self.change_handle(&proposal_ref),
                    "version": version,
                    "commit": commit,
                    "receipt": receipt
                }))
            }
            ForgeOperation::ReflectDisposition => {
                let work_unit = input.get("work_unit").cloned().unwrap_or(Value::Null);
                let change = input.get("change").cloned().unwrap_or(Value::Null);
                let disposition = required_str(&input, "disposition")?;
                let body = required_str(&input, "body")?;
                let number = self.ticket_number_from_handle(&work_unit)?;
                let receipt = self.comment_ticket(number, &format!("{disposition}\n\n{body}"))?;
                Ok(
                    json!({"work_unit": work_unit, "change": change, "disposition": disposition, "receipt": receipt}),
                )
            }
            ForgeOperation::ApplyApprovedChange => {
                let work_unit = input.get("work_unit").cloned().unwrap_or(Value::Null);
                let change = input.get("change").cloned().unwrap_or(Value::Null);
                let commit = required_str(&input, "approved_commit")?;
                let number = self.ticket_number_from_handle(&work_unit)?;
                let receipt = self.comment_ticket(
                    number,
                    &format!("Approved change applied at commit {commit}."),
                )?;
                Ok(
                    json!({"work_unit": work_unit, "change": change, "applied_commit": commit, "receipt": receipt}),
                )
            }
            ForgeOperation::CloseOut => {
                let work_unit = input.get("work_unit").cloned().unwrap_or(Value::Null);
                let number = self.ticket_number_from_handle(&work_unit)?;
                let completion = required_str(&input, "completion")?;
                let body = required_str(&input, "body")?;
                let receipt = self.comment_ticket(number, body)?;
                let _ = self.graphql(
                    r#"
                    mutation CloseTicket($trackerId: Int!, $number: Int!) {
                      updateTicket(trackerId: $trackerId, number: $number, input: { status: RESOLVED }) { ticket { number } }
                    }
                    "#,
                    json!({ "trackerId": self.config.tracker_id, "number": number }),
                )?;
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

fn resolve_token(config: Option<&CredentialConfig>) -> Result<String, ForgeError> {
    let Some(config) = config else {
        return Err(ForgeError::new(
            "sourcehut connector requires deployment credential source",
        ));
    };
    if let Some(env_name) = config.env.as_deref() {
        return std::env::var(env_name)
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
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    Err(ForgeError::new(
        "sourcehut connector credential source must declare env or command",
    ))
}
