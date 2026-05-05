//! Session transcript event writing shared by runa binaries.
//!
//! Transcript capture is opt-in through `RUNA_TRANSCRIPT_DIR`. When enabled,
//! each producer appends JSON Lines events to `events.jsonl` in that directory.

use serde::Serialize;
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub const TRANSCRIPT_DIR_ENV: &str = "RUNA_TRANSCRIPT_DIR";
pub const REDACT_ENV_ENV: &str = "RUNA_TRANSCRIPT_REDACT_ENV";
pub const EVENTS_FILE_NAME: &str = "events.jsonl";

#[derive(Debug, Default)]
pub struct TranscriptEvent<'a> {
    pub source: &'a str,
    pub kind: &'a str,
    pub protocol: Option<&'a str>,
    pub work_unit: Option<&'a str>,
    pub stream: Option<&'a str>,
    pub tool_name: Option<&'a str>,
    pub content: Option<&'a str>,
    pub exit_code: Option<i32>,
    pub success: Option<bool>,
    pub payload: Option<Value>,
}

#[derive(Serialize)]
struct SerializableTranscriptEvent<'a> {
    schema_version: u32,
    timestamp_ms: u64,
    source: &'a str,
    kind: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    protocol: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    work_unit: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    success: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    payload: Option<Value>,
}

pub fn append_event(event: TranscriptEvent<'_>) -> io::Result<()> {
    let Some(dir) = transcript_dir() else {
        return Ok(());
    };
    fs::create_dir_all(&dir)?;

    let redactions = redactions_from_env();
    let content = event
        .content
        .map(|content| redact_text(content, &redactions));
    let payload = event
        .payload
        .map(|payload| redact_value(payload, &redactions));
    let serializable = SerializableTranscriptEvent {
        schema_version: 1,
        timestamp_ms: timestamp_ms(),
        source: event.source,
        kind: event.kind,
        protocol: event.protocol,
        work_unit: event.work_unit,
        stream: event.stream,
        tool_name: event.tool_name,
        content,
        exit_code: event.exit_code,
        success: event.success,
        payload,
    };

    let mut payload = serde_json::to_vec(&serializable).map_err(io::Error::other)?;
    payload.push(b'\n');

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(EVENTS_FILE_NAME))?;
    file.write_all(&payload)?;
    file.flush()
}

pub fn capture_enabled() -> bool {
    transcript_dir().is_some()
}

pub fn transcript_env() -> Vec<(String, String)> {
    [TRANSCRIPT_DIR_ENV, REDACT_ENV_ENV]
        .into_iter()
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.is_empty())
                .map(|value| (name.to_string(), value))
        })
        .collect()
}

fn transcript_dir() -> Option<PathBuf> {
    std::env::var_os(TRANSCRIPT_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn redactions_from_env() -> Vec<(String, String)> {
    let Ok(names) = std::env::var(REDACT_ENV_ENV) else {
        return Vec::new();
    };

    names
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.is_empty())
                .map(|value| (name.to_string(), value))
        })
        .collect()
}

fn redact_value(value: Value, redactions: &[(String, String)]) -> Value {
    match value {
        Value::String(value) => Value::String(redact_text(&value, redactions)),
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(|value| redact_value(value, redactions))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| (key, redact_value(value, redactions)))
                .collect(),
        ),
        other => other,
    }
}

fn redact_text(text: &str, redactions: &[(String, String)]) -> String {
    let mut redacted = text.to_string();
    for (name, value) in redactions {
        redacted = redacted.replace(value, &format!("[REDACTED:{name}]"));
    }
    redacted
}

fn timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{redact_text, redact_value};
    use serde_json::json;

    #[test]
    fn redaction_replaces_configured_secret_values_in_text() {
        let redacted = redact_text(
            "token secret-value",
            &[("TOKEN".to_string(), "secret-value".to_string())],
        );

        assert_eq!(redacted, "token [REDACTED:TOKEN]");
    }

    #[test]
    fn redaction_replaces_configured_secret_values_in_json_strings() {
        let redacted = redact_value(
            json!({"nested": ["secret-value"]}),
            &[("TOKEN".to_string(), "secret-value".to_string())],
        );

        assert_eq!(redacted, json!({"nested": ["[REDACTED:TOKEN]"]}));
    }
}
