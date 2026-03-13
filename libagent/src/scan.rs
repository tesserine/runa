use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::store::{ArtifactStore, StoreError, ValidationStatus, content_hash, raw_content_hash};
use crate::validation::Violation;

struct ArtifactInput<'a> {
    artifact_type: &'a str,
    instance_id: &'a str,
    path: &'a Path,
}

impl ArtifactInput<'_> {
    fn as_ref(&self) -> ArtifactRef {
        ArtifactRef {
            artifact_type: self.artifact_type.to_string(),
            instance_id: self.instance_id.to_string(),
            path: self.path.to_path_buf(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArtifactRef {
    pub artifact_type: String,
    pub instance_id: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InvalidArtifact {
    pub artifact_type: String,
    pub instance_id: String,
    pub path: PathBuf,
    pub violations: Vec<Violation>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MalformedArtifact {
    pub artifact_type: String,
    pub instance_id: String,
    pub path: PathBuf,
    pub error: String,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct ScanResult {
    pub new: Vec<ArtifactRef>,
    pub modified: Vec<ArtifactRef>,
    pub invalid: Vec<InvalidArtifact>,
    pub malformed: Vec<MalformedArtifact>,
    pub removed: Vec<ArtifactRef>,
    pub unrecognized_dirs: Vec<String>,
}

#[derive(Debug)]
pub enum ScanError {
    Io(std::io::Error),
    Store(StoreError),
}

impl fmt::Display for ScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScanError::Io(err) => write!(f, "I/O error: {err}"),
            ScanError::Store(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ScanError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ScanError::Io(err) => Some(err),
            ScanError::Store(err) => Some(err),
        }
    }
}

pub fn scan(workspace_dir: &Path, store: &mut ArtifactStore) -> Result<ScanResult, ScanError> {
    let scan_timestamp_ms = current_time_ms();
    let known_types: HashSet<String> = store
        .artifact_type_names()
        .into_iter()
        .map(str::to_string)
        .collect();
    let existing_keys = store.all_instance_keys();
    let mut seen_keys = HashSet::new();
    let mut result = ScanResult::default();

    if workspace_dir.exists() {
        for entry in std::fs::read_dir(workspace_dir).map_err(ScanError::Io)? {
            let entry = entry.map_err(ScanError::Io)?;
            let file_type = entry.file_type().map_err(ScanError::Io)?;
            if !file_type.is_dir() {
                continue;
            }

            let type_name = entry.file_name().to_string_lossy().into_owned();
            if !known_types.contains(&type_name) {
                result.unrecognized_dirs.push(type_name);
                continue;
            }

            scan_type_dir(
                &entry.path(),
                &type_name,
                store,
                scan_timestamp_ms,
                &mut seen_keys,
                &mut result,
            )?;
        }
    }

    result.unrecognized_dirs.sort();

    for (artifact_type, instance_id) in existing_keys {
        if seen_keys.contains(&(artifact_type.clone(), instance_id.clone())) {
            continue;
        }

        let Some(state) = store.get(&artifact_type, &instance_id).cloned() else {
            continue;
        };
        store
            .remove(&artifact_type, &instance_id)
            .map_err(ScanError::Store)?;
        result.removed.push(ArtifactRef {
            artifact_type,
            instance_id,
            path: state.path,
        });
    }

    result.removed.sort_by(|left, right| {
        (&left.artifact_type, &left.instance_id).cmp(&(&right.artifact_type, &right.instance_id))
    });

    Ok(result)
}

fn scan_type_dir(
    type_dir: &Path,
    artifact_type: &str,
    store: &mut ArtifactStore,
    scan_timestamp_ms: u64,
    seen_keys: &mut HashSet<(String, String)>,
    result: &mut ScanResult,
) -> Result<(), ScanError> {
    for entry in std::fs::read_dir(type_dir).map_err(ScanError::Io)? {
        let entry = entry.map_err(ScanError::Io)?;
        let file_type = entry.file_type().map_err(ScanError::Io)?;
        if !file_type.is_file() {
            continue;
        }

        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }

        let instance_id = path
            .file_stem()
            .expect("JSON file has a filename stem")
            .to_string_lossy()
            .into_owned();
        seen_keys.insert((artifact_type.to_string(), instance_id.clone()));

        let existing = store.get(artifact_type, &instance_id).cloned();
        let bytes = std::fs::read(&path).map_err(ScanError::Io)?;

        match serde_json::from_slice::<Value>(&bytes) {
            Ok(data) => handle_json_artifact(
                ArtifactInput {
                    artifact_type,
                    instance_id: &instance_id,
                    path: &path,
                },
                &data,
                existing.as_ref(),
                store,
                scan_timestamp_ms,
                result,
            )?,
            Err(err) => handle_malformed_artifact(
                ArtifactInput {
                    artifact_type,
                    instance_id: &instance_id,
                    path: &path,
                },
                &bytes,
                err.to_string(),
                existing.as_ref(),
                store,
                scan_timestamp_ms,
                result,
            )?,
        }
    }

    Ok(())
}

fn handle_json_artifact(
    artifact: ArtifactInput<'_>,
    data: &Value,
    existing: Option<&crate::store::ArtifactState>,
    store: &mut ArtifactStore,
    scan_timestamp_ms: u64,
    result: &mut ScanResult,
) -> Result<(), ScanError> {
    let new_hash = content_hash(data);
    let change_ref = classify_change(&artifact, existing, &new_hash);

    if let Some(change_ref) = change_ref {
        store
            .record_with_timestamp(
                artifact.artifact_type,
                artifact.instance_id,
                artifact.path,
                data,
                scan_timestamp_ms,
            )
            .map_err(ScanError::Store)?;
        push_change(result, existing.is_some(), change_ref);
    }

    let Some(state) = store.get(artifact.artifact_type, artifact.instance_id) else {
        return Ok(());
    };

    if let ValidationStatus::Invalid(violations) = &state.status {
        result.invalid.push(InvalidArtifact {
            artifact_type: artifact.artifact_type.to_string(),
            instance_id: artifact.instance_id.to_string(),
            path: artifact.path.to_path_buf(),
            violations: violations.clone(),
        });
    }

    Ok(())
}

fn handle_malformed_artifact(
    artifact: ArtifactInput<'_>,
    bytes: &[u8],
    error: String,
    existing: Option<&crate::store::ArtifactState>,
    store: &mut ArtifactStore,
    scan_timestamp_ms: u64,
    result: &mut ScanResult,
) -> Result<(), ScanError> {
    let new_hash = raw_content_hash(bytes);
    let change_ref = classify_change(&artifact, existing, &new_hash);

    if let Some(change_ref) = change_ref {
        store
            .record_malformed_with_timestamp(
                artifact.artifact_type,
                artifact.instance_id,
                artifact.path,
                bytes,
                error.clone(),
                scan_timestamp_ms,
            )
            .map_err(ScanError::Store)?;
        push_change(result, existing.is_some(), change_ref);
    }

    result.malformed.push(MalformedArtifact {
        artifact_type: artifact.artifact_type.to_string(),
        instance_id: artifact.instance_id.to_string(),
        path: artifact.path.to_path_buf(),
        error,
    });

    Ok(())
}

fn classify_change(
    artifact: &ArtifactInput<'_>,
    existing: Option<&crate::store::ArtifactState>,
    new_hash: &str,
) -> Option<ArtifactRef> {
    match existing {
        None => Some(artifact.as_ref()),
        Some(state) if state.content_hash != new_hash => Some(artifact.as_ref()),
        Some(_) => None,
    }
}

fn push_change(result: &mut ScanResult, was_present: bool, artifact: ArtifactRef) {
    if was_present {
        result.modified.push(artifact);
    } else {
        result.new.push(artifact);
    }
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::make_store;
    use serde_json::json;
    use tempfile::TempDir;

    fn write_file(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn empty_dir_returns_empty_result() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let mut store = make_store(&tmp.path().join("store"), vec!["report"]);

        let result = scan(&workspace, &mut store).unwrap();

        assert_eq!(result, ScanResult::default());
    }

    #[test]
    fn valid_artifact_is_recorded_as_new() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        let artifact_path = workspace.join("report/alpha.json");
        write_file(&artifact_path, r#"{"title":"ok"}"#);
        let mut store = make_store(&tmp.path().join("store"), vec!["report"]);

        let result = scan(&workspace, &mut store).unwrap();

        assert_eq!(result.new.len(), 1);
        assert!(result.modified.is_empty());
        assert!(result.invalid.is_empty());
        assert!(result.malformed.is_empty());
        assert_eq!(
            store.get("report", "alpha").unwrap().status,
            ValidationStatus::Valid
        );
    }

    #[test]
    fn invalid_artifact_is_reported_and_recorded() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        write_file(&workspace.join("report/bad.json"), r#"{"score":1}"#);
        let mut store = make_store(&tmp.path().join("store"), vec!["report"]);

        let result = scan(&workspace, &mut store).unwrap();

        assert_eq!(result.new.len(), 1);
        assert_eq!(result.invalid.len(), 1);
        assert!(matches!(
            store.get("report", "bad").unwrap().status,
            ValidationStatus::Invalid(_)
        ));
    }

    #[test]
    fn malformed_artifact_is_reported_and_recorded() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        write_file(&workspace.join("report/bad.json"), r#"{ nope }"#);
        let mut store = make_store(&tmp.path().join("store"), vec!["report"]);

        let result = scan(&workspace, &mut store).unwrap();

        assert_eq!(result.new.len(), 1);
        assert_eq!(result.malformed.len(), 1);
        assert!(matches!(
            store.get("report", "bad").unwrap().status,
            ValidationStatus::Malformed(_)
        ));
    }

    #[test]
    fn modified_artifact_updates_timestamp_and_reports_modified() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        let artifact_path = workspace.join("report/item.json");
        write_file(&artifact_path, r#"{"title":"first"}"#);
        let mut store = make_store(&tmp.path().join("store"), vec!["report"]);

        store
            .record_with_timestamp(
                "report",
                "item",
                &artifact_path,
                &json!({"title": "old"}),
                1000,
            )
            .unwrap();

        let result = scan(&workspace, &mut store).unwrap();

        assert_eq!(result.modified.len(), 1);
        assert!(store.get("report", "item").unwrap().last_modified_ms > 1000);
    }

    #[test]
    fn unrecognized_dir_is_reported() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(workspace.join("unknown")).unwrap();
        let mut store = make_store(&tmp.path().join("store"), vec!["report"]);

        let result = scan(&workspace, &mut store).unwrap();

        assert_eq!(result.unrecognized_dirs, vec!["unknown".to_string()]);
    }

    #[test]
    fn removed_artifact_is_deleted_from_store() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let store_dir = tmp.path().join("store");
        let mut store = make_store(&store_dir, vec!["report"]);

        store
            .record(
                "report",
                "old",
                Path::new("report/old.json"),
                &json!({"title": "old"}),
            )
            .unwrap();

        let result = scan(&workspace, &mut store).unwrap();

        assert_eq!(result.removed.len(), 1);
        assert!(store.get("report", "old").is_none());
        assert!(!store_dir.join("report/old.json").exists());
    }

    #[test]
    fn idempotent_scan_has_no_changes_on_second_run() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        write_file(&workspace.join("report/alpha.json"), r#"{"title":"ok"}"#);
        let mut store = make_store(&tmp.path().join("store"), vec!["report"]);

        let first = scan(&workspace, &mut store).unwrap();
        let before = store.get("report", "alpha").unwrap().last_modified_ms;
        let second = scan(&workspace, &mut store).unwrap();
        let after = store.get("report", "alpha").unwrap().last_modified_ms;

        assert_eq!(first.new.len(), 1);
        assert_eq!(second, ScanResult::default());
        assert_eq!(before, after);
    }

    #[test]
    fn non_json_files_are_ignored() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        write_file(&workspace.join("report/readme.txt"), "ignore");
        let mut store = make_store(&tmp.path().join("store"), vec!["report"]);

        let result = scan(&workspace, &mut store).unwrap();

        assert_eq!(result, ScanResult::default());
    }
}
