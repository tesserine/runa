//! Session transcript event writing shared by runa binaries.
//!
//! Transcript capture is opt-in through `RUNA_TRANSCRIPT_DIR`. When enabled,
//! each producer appends JSON Lines events to `events.jsonl` in that directory.

use serde::Serialize;
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::project::TranscriptConfig;

pub const TRANSCRIPT_DIR_ENV: &str = "RUNA_TRANSCRIPT_DIR";
pub const REDACT_ENV_ENV: &str = "RUNA_TRANSCRIPT_REDACT_ENV";
pub const EVENTS_FILE_NAME: &str = "events.jsonl";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TranscriptSettings {
    pub dir: Option<PathBuf>,
    pub redact_env: Vec<String>,
}

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
    let settings = TranscriptSettings {
        dir: transcript_dir(),
        redact_env: std::env::var(REDACT_ENV_ENV)
            .ok()
            .map(|names| split_redaction_names(&names))
            .unwrap_or_default(),
    };
    append_event_with_settings(event, &settings)
}

pub fn append_event_with_settings(
    event: TranscriptEvent<'_>,
    settings: &TranscriptSettings,
) -> io::Result<()> {
    let Some(dir) = settings.dir.as_ref() else {
        return Ok(());
    };
    fs::create_dir_all(dir)?;

    let redactions = redactions_from_names(settings.redact_env.clone());
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

pub fn capture_enabled_with_settings(settings: &TranscriptSettings) -> bool {
    settings.dir.is_some()
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

pub fn transcript_env_from_settings(settings: &TranscriptSettings) -> Vec<(String, String)> {
    let mut env = Vec::new();
    if let Some(dir) = &settings.dir {
        env.push((
            TRANSCRIPT_DIR_ENV.to_string(),
            dir.to_string_lossy().into_owned(),
        ));
    }
    if !settings.redact_env.is_empty() {
        env.push((REDACT_ENV_ENV.to_string(), settings.redact_env.join(",")));
    }
    env
}

pub fn resolve_transcript_settings(
    working_dir: &Path,
    config: &TranscriptConfig,
) -> TranscriptSettings {
    let dir = std::env::var_os(TRANSCRIPT_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            config
                .dir
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .map(|path| {
                    if path.is_absolute() {
                        path
                    } else {
                        working_dir.join(path)
                    }
                })
        });
    let redact_env = match std::env::var(REDACT_ENV_ENV) {
        Ok(names) if !names.is_empty() => split_redaction_names(&names),
        _ => config
            .redact_env
            .iter()
            .map(|name| name.trim())
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect(),
    };

    TranscriptSettings { dir, redact_env }
}

fn transcript_dir() -> Option<PathBuf> {
    std::env::var_os(TRANSCRIPT_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn split_redaction_names(names: &str) -> Vec<String> {
    names
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

fn redactions_from_names(names: Vec<String>) -> Vec<(String, String)> {
    names
        .into_iter()
        .filter_map(|name| {
            std::env::var(&name)
                .ok()
                .filter(|value| !value.is_empty())
                .map(|value| (name, value))
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
    use super::{
        EVENTS_FILE_NAME, REDACT_ENV_ENV, TRANSCRIPT_DIR_ENV, TranscriptEvent, TranscriptSettings,
        append_event_with_settings, redact_text, redact_value, resolve_transcript_settings,
        transcript_env_from_settings,
    };
    use crate::project::TranscriptConfig;
    use crate::test_helpers::EnvGuard;
    use serde_json::json;
    use std::path::PathBuf;

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

    #[test]
    fn transcript_settings_resolve_config_values_when_environment_is_unset() {
        let _env = EnvGuard::unset(&[TRANSCRIPT_DIR_ENV, REDACT_ENV_ENV]);
        let temp = tempfile::tempdir().unwrap();
        let working_dir = temp.path().join("project");
        let config = TranscriptConfig {
            dir: Some("transcripts".to_string()),
            redact_env: vec!["SECRET_TOKEN".to_string()],
        };

        let settings = resolve_transcript_settings(&working_dir, &config);

        assert_eq!(settings.dir, Some(working_dir.join("transcripts")));
        assert_eq!(settings.redact_env, ["SECRET_TOKEN"]);
    }

    #[test]
    fn transcript_settings_resolve_environment_values_over_config_values() {
        let _env = EnvGuard::set(&[
            (TRANSCRIPT_DIR_ENV, "/tmp/env-transcripts"),
            (REDACT_ENV_ENV, "ENV_TOKEN"),
        ]);
        let temp = tempfile::tempdir().unwrap();
        let config = TranscriptConfig {
            dir: Some("config-transcripts".to_string()),
            redact_env: vec!["CONFIG_TOKEN".to_string()],
        };

        let settings = resolve_transcript_settings(temp.path(), &config);

        assert_eq!(settings.dir, Some(PathBuf::from("/tmp/env-transcripts")));
        assert_eq!(settings.redact_env, ["ENV_TOKEN"]);
    }

    #[test]
    fn transcript_env_from_settings_exports_config_resolved_values() {
        let settings = TranscriptSettings {
            dir: Some(PathBuf::from("/tmp/runa-transcript")),
            redact_env: vec!["SECRET_TOKEN".to_string(), "API_KEY".to_string()],
        };

        let env = transcript_env_from_settings(&settings);

        assert_eq!(
            env,
            vec![
                (
                    TRANSCRIPT_DIR_ENV.to_string(),
                    "/tmp/runa-transcript".to_string()
                ),
                (
                    REDACT_ENV_ENV.to_string(),
                    "SECRET_TOKEN,API_KEY".to_string()
                )
            ]
        );
    }

    #[test]
    fn append_event_with_settings_writes_and_redacts_using_config_resolved_settings() {
        let _env = EnvGuard::unset_and_set(
            &[TRANSCRIPT_DIR_ENV, REDACT_ENV_ENV],
            &[("SECRET_TOKEN", "SECRET_VALUE")],
        );
        let temp = tempfile::tempdir().unwrap();
        let transcript_dir = temp.path().join("transcript");
        let settings = TranscriptSettings {
            dir: Some(transcript_dir.clone()),
            redact_env: vec!["SECRET_TOKEN".to_string()],
        };

        append_event_with_settings(
            TranscriptEvent {
                source: "test",
                kind: "event",
                content: Some("hide SECRET_VALUE"),
                ..Default::default()
            },
            &settings,
        )
        .unwrap();

        let events = std::fs::read_to_string(transcript_dir.join(EVENTS_FILE_NAME)).unwrap();
        assert!(events.contains("[REDACTED:SECRET_TOKEN]"), "{events}");
        assert!(!events.contains("SECRET_VALUE"), "{events}");
    }
}
