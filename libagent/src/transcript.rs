//! Session transcript event writing shared by runa binaries.
//!
//! Transcript capture is opt-in through `RUNA_TRANSCRIPT_DIR`. When enabled,
//! producers append JSON Lines events under the configured root, separated by
//! deployment, work unit, and run.

use serde::Serialize;
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::forge_address::ForgeProject;
use crate::project::{DeploymentConfig, TranscriptConfig};

pub const TRANSCRIPT_DIR_ENV: &str = "RUNA_TRANSCRIPT_DIR";
pub const REDACT_ENV_ENV: &str = "RUNA_TRANSCRIPT_REDACT_ENV";
pub const DEPLOYMENT_ENV: &str = "RUNA_TRANSCRIPT_DEPLOYMENT";
pub const RUN_ID_ENV: &str = "RUNA_TRANSCRIPT_RUN_ID";
pub const EVENTS_FILE_NAME: &str = "events.jsonl";
const UNSCOPED_WORK_UNIT_COMPONENT: &str = "_unscoped";
const UNKNOWN_DEPLOYMENT: &str = "unknown";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TranscriptSettings {
    pub dir: Option<PathBuf>,
    pub redact_env: Vec<String>,
    pub deployment: Option<String>,
    pub run_id: Option<String>,
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
    deployment: &'a str,
    run_id: &'a str,
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
        deployment: std::env::var(DEPLOYMENT_ENV)
            .ok()
            .filter(|value| !value.is_empty()),
        run_id: std::env::var(RUN_ID_ENV)
            .ok()
            .filter(|value| !value.is_empty()),
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
    let deployment = settings
        .deployment
        .as_deref()
        .filter(|value| !value.is_empty())
        .unwrap_or(UNKNOWN_DEPLOYMENT);
    let run_id = match settings.run_id.as_deref().filter(|value| !value.is_empty()) {
        Some(run_id) => run_id,
        None => process_run_id(),
    };
    let event_path = event_file_path_for(dir, deployment, event.work_unit, run_id);
    if let Some(parent) = event_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let redactions = redactions_from_names(settings.redact_env.clone());
    let content = event
        .content
        .map(|content| redact_text(content, &redactions));
    let payload = event
        .payload
        .map(|payload| redact_value(payload, &redactions));
    let serializable = SerializableTranscriptEvent {
        schema_version: 2,
        timestamp_ms: timestamp_ms(),
        deployment,
        run_id,
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
        .open(event_path)?;
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
    [
        TRANSCRIPT_DIR_ENV,
        REDACT_ENV_ENV,
        DEPLOYMENT_ENV,
        RUN_ID_ENV,
    ]
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
    if settings.dir.is_some() {
        if let Some(deployment) = &settings.deployment
            && !deployment.is_empty()
        {
            env.push((DEPLOYMENT_ENV.to_string(), deployment.clone()));
        }
        if let Some(run_id) = &settings.run_id
            && !run_id.is_empty()
        {
            env.push((RUN_ID_ENV.to_string(), run_id.clone()));
        }
    }
    env
}

pub fn resolve_transcript_settings(
    working_dir: &Path,
    config: &TranscriptConfig,
) -> TranscriptSettings {
    resolve_transcript_settings_with_forge(
        working_dir,
        config,
        &DeploymentConfig::default(),
        &ForgeProject::default(),
    )
}

pub fn resolve_transcript_settings_with_forge(
    working_dir: &Path,
    config: &TranscriptConfig,
    deployment: &DeploymentConfig,
    forge: &ForgeProject,
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
    let deployment = std::env::var(DEPLOYMENT_ENV)
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| deployment_identity(working_dir, deployment, forge));
    let run_id = std::env::var(RUN_ID_ENV)
        .ok()
        .filter(|value| !value.is_empty());

    TranscriptSettings {
        dir,
        redact_env,
        deployment,
        run_id,
    }
}

pub fn with_run_id(settings: &TranscriptSettings, run_id: impl Into<String>) -> TranscriptSettings {
    let mut settings = settings.clone();
    settings.run_id = Some(run_id.into());
    settings
}

pub fn new_run_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("run-{nanos}-{}", std::process::id())
}

pub fn event_file_path(settings: &TranscriptSettings, work_unit: Option<&str>) -> Option<PathBuf> {
    let dir = settings.dir.as_ref()?;
    let deployment = settings.deployment.as_deref().unwrap_or(UNKNOWN_DEPLOYMENT);
    let run_id = match settings.run_id.as_deref().filter(|value| !value.is_empty()) {
        Some(run_id) => run_id,
        None => process_run_id(),
    };
    Some(event_file_path_for(dir, deployment, work_unit, run_id))
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

fn deployment_identity(
    working_dir: &Path,
    deployment: &DeploymentConfig,
    forge: &ForgeProject,
) -> Option<String> {
    if forge.repositories.is_empty() {
        return Some(project_deployment_identity(working_dir));
    }
    forge
        .deployment_identity(deployment.repository.as_deref())
        .ok()
        .or_else(|| Some(project_deployment_identity(working_dir)))
}

fn project_deployment_identity(working_dir: &Path) -> String {
    let canonical = working_dir
        .canonicalize()
        .unwrap_or_else(|_| working_dir.to_path_buf());
    let mut hasher = Sha256::new();
    hasher.update(canonical.to_string_lossy().as_bytes());
    let digest = hasher.finalize();
    format!("project:sha256:{digest:x}")
}

fn event_file_path_for(
    root: &Path,
    deployment: &str,
    work_unit: Option<&str>,
    run_id: &str,
) -> PathBuf {
    let work_unit = work_unit.unwrap_or(UNSCOPED_WORK_UNIT_COMPONENT);
    root.join("deployments")
        .join(encode_path_component(deployment))
        .join("work-units")
        .join(encode_path_component(work_unit))
        .join("runs")
        .join(encode_path_component(run_id))
        .join(EVENTS_FILE_NAME)
}

fn encode_path_component(value: &str) -> String {
    if value.is_empty() {
        return "_empty".to_string();
    }

    let mut encoded = String::new();
    let only_dots = value.as_bytes().iter().all(|byte| *byte == b'.');
    for byte in value.as_bytes() {
        match byte {
            b'.' if only_dots => encoded.push_str("%2E"),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                encoded.push(char::from(*byte));
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn process_run_id() -> &'static str {
    static RUN_ID: OnceLock<String> = OnceLock::new();
    RUN_ID.get_or_init(new_run_id).as_str()
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
        DEPLOYMENT_ENV, EVENTS_FILE_NAME, REDACT_ENV_ENV, RUN_ID_ENV, TRANSCRIPT_DIR_ENV,
        TranscriptEvent, TranscriptSettings, UNSCOPED_WORK_UNIT_COMPONENT, append_event,
        append_event_with_settings, encode_path_component, event_file_path, redact_text,
        redact_value, resolve_transcript_settings, resolve_transcript_settings_with_forge,
        transcript_env_from_settings,
    };
    use crate::forge_address::{ForgeProject, RawForgeInstance, RawForges, RawRepository};
    use crate::project::{DeploymentConfig, TranscriptConfig};
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
        let _env = EnvGuard::unset(&[
            TRANSCRIPT_DIR_ENV,
            REDACT_ENV_ENV,
            DEPLOYMENT_ENV,
            RUN_ID_ENV,
        ]);
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
            (DEPLOYMENT_ENV, "github:env/project"),
            (RUN_ID_ENV, "run-env"),
        ]);
        let temp = tempfile::tempdir().unwrap();
        let config = TranscriptConfig {
            dir: Some("config-transcripts".to_string()),
            redact_env: vec!["CONFIG_TOKEN".to_string()],
        };

        let settings = resolve_transcript_settings(temp.path(), &config);

        assert_eq!(settings.dir, Some(PathBuf::from("/tmp/env-transcripts")));
        assert_eq!(settings.redact_env, ["ENV_TOKEN"]);
        assert_eq!(settings.deployment.as_deref(), Some("github:env/project"));
        assert_eq!(settings.run_id.as_deref(), Some("run-env"));
    }

    #[test]
    fn transcript_env_from_settings_exports_config_resolved_values() {
        let settings = TranscriptSettings {
            dir: Some(PathBuf::from("/tmp/runa-transcript")),
            redact_env: vec!["SECRET_TOKEN".to_string(), "API_KEY".to_string()],
            deployment: Some("github:tesserine/runa".to_string()),
            run_id: Some("run-1".to_string()),
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
                ),
                (
                    DEPLOYMENT_ENV.to_string(),
                    "github:tesserine/runa".to_string()
                ),
                (RUN_ID_ENV.to_string(), "run-1".to_string())
            ]
        );
    }

    #[test]
    fn append_event_with_settings_writes_and_redacts_using_config_resolved_settings() {
        let _env = EnvGuard::unset_and_set(
            &[
                TRANSCRIPT_DIR_ENV,
                REDACT_ENV_ENV,
                DEPLOYMENT_ENV,
                RUN_ID_ENV,
            ],
            &[("SECRET_TOKEN", "SECRET_VALUE")],
        );
        let temp = tempfile::tempdir().unwrap();
        let transcript_dir = temp.path().join("transcript");
        let settings = TranscriptSettings {
            dir: Some(transcript_dir.clone()),
            redact_env: vec!["SECRET_TOKEN".to_string()],
            deployment: Some("github:tesserine/runa".to_string()),
            run_id: Some("run-1".to_string()),
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

        let events_path = event_file_path(&settings, None).unwrap();
        assert!(events_path.ends_with(EVENTS_FILE_NAME));
        assert!(
            !transcript_dir.join(EVENTS_FILE_NAME).exists(),
            "events should be routed below the transcript root"
        );
        let events = std::fs::read_to_string(events_path).unwrap();
        assert!(events.contains("[REDACTED:SECRET_TOKEN]"), "{events}");
        assert!(!events.contains("SECRET_VALUE"), "{events}");
        assert!(events.contains(r#""schema_version":2"#), "{events}");
        assert!(
            events.contains(r#""deployment":"github:tesserine/runa""#),
            "{events}"
        );
        assert!(events.contains(r#""run_id":"run-1""#), "{events}");
    }

    #[test]
    fn append_event_neutralizes_traversal_deployment_components() {
        let temp = tempfile::tempdir().unwrap();
        let transcript_dir = temp.path().join("transcript");
        let transcript_dir_string = transcript_dir.to_string_lossy().into_owned();
        let _env = EnvGuard::unset_and_set(
            &[REDACT_ENV_ENV],
            &[
                (TRANSCRIPT_DIR_ENV, &transcript_dir_string),
                (DEPLOYMENT_ENV, ".."),
                (RUN_ID_ENV, "run-1"),
            ],
        );

        append_event(TranscriptEvent {
            source: "test",
            kind: "event",
            content: Some("content"),
            ..Default::default()
        })
        .unwrap();

        let expected_path = transcript_dir
            .join("deployments")
            .join("%2E%2E")
            .join("work-units")
            .join(UNSCOPED_WORK_UNIT_COMPONENT)
            .join("runs")
            .join("run-1")
            .join(EVENTS_FILE_NAME);
        assert!(
            expected_path.exists(),
            "event should be written under encoded deployment path: {expected_path:?}"
        );
        let expected_parent = expected_path.parent().unwrap().canonicalize().unwrap();
        let canonical_root = transcript_dir.canonicalize().unwrap();
        assert!(
            expected_parent.starts_with(&canonical_root),
            "event path should remain inside transcript root: {expected_parent:?}"
        );
        assert!(
            !transcript_dir.join("work-units").exists(),
            "deployment traversal must not collapse back to the transcript root"
        );
        assert!(
            !transcript_dir.join(EVENTS_FILE_NAME).exists(),
            "event should not be written at the transcript root"
        );
    }

    #[test]
    fn transcript_path_routes_by_deployment_work_unit_and_run() {
        let settings = TranscriptSettings {
            dir: Some(PathBuf::from("/tmp/runa-transcript")),
            deployment: Some("github:tesserine/runa".to_string()),
            run_id: Some("run:one".to_string()),
            ..Default::default()
        };

        let path = event_file_path(&settings, Some("work/unit:1")).unwrap();
        let path = path.to_string_lossy();

        assert!(path.contains("deployments/github%3Atesserine%2Fruna/"));
        assert!(path.contains("work-units/work%2Funit%3A1/"));
        assert!(path.contains("runs/run%3Aone/events.jsonl"));

        let other_run = TranscriptSettings {
            run_id: Some("run:two".to_string()),
            ..settings
        };
        assert_ne!(
            event_file_path(&other_run, Some("work/unit:1")),
            event_file_path(&other_run, Some("work/unit:2"))
        );
        assert_ne!(
            event_file_path(&other_run, Some("work/unit:1")),
            event_file_path(
                &TranscriptSettings {
                    run_id: Some("run:three".to_string()),
                    ..other_run
                },
                Some("work/unit:1")
            )
        );
    }

    #[test]
    fn encode_path_component_preserves_existing_encoding_and_neutralizes_traversal() {
        let cases = [
            ("", "_empty"),
            (".", "%2E"),
            ("..", "%2E%2E"),
            ("...", "%2E%2E%2E"),
            ("github:tesserine/alpha", "github%3Atesserine%2Falpha"),
            ("work.unit", "work.unit"),
            ("work/unit:1", "work%2Funit%3A1"),
        ];

        for (input, expected) in cases {
            let encoded = encode_path_component(input);
            assert_eq!(encoded, expected);
            let components = std::path::Path::new(&encoded)
                .components()
                .collect::<Vec<_>>();
            assert!(
                matches!(components.as_slice(), [std::path::Component::Normal(_)]),
                "encoded path component should not navigate directories: {input:?} -> {encoded:?}"
            );
        }
    }

    #[test]
    fn transcript_settings_derives_deployment_from_forge_identity() {
        let _env = EnvGuard::unset(&[
            TRANSCRIPT_DIR_ENV,
            REDACT_ENV_ENV,
            DEPLOYMENT_ENV,
            RUN_ID_ENV,
        ]);
        let temp = tempfile::tempdir().unwrap();
        let forge = ForgeProject::resolve(RawForges {
            instances: vec![RawForgeInstance {
                id: "github-com".to_string(),
                forge_type: "github".to_string(),
                host: Some("github.com".to_string()),
                git_host: None,
                tracker_host: None,
            }],
            repositories: vec![RawRepository {
                id: "runa".to_string(),
                instance: "github-com".to_string(),
                owner: "tesserine".to_string(),
                name: "runa".to_string(),
            }],
            trackers: Vec::new(),
        })
        .unwrap();
        let settings = resolve_transcript_settings_with_forge(
            temp.path(),
            &TranscriptConfig::default(),
            &DeploymentConfig::default(),
            &forge,
        );

        assert_eq!(
            settings.deployment.as_deref(),
            Some("github@github.com/repo/tesserine/runa")
        );
    }
}
