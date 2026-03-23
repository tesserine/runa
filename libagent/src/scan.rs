use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::store::{
    ArtifactState, ArtifactStore, StoreError, ValidationStatus, content_hash, raw_content_hash,
};
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

#[derive(Debug, Clone, PartialEq)]
pub struct UnreadableArtifact {
    pub path: PathBuf,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PartiallyScannedType {
    pub artifact_type: String,
    pub unreadable_entries: usize,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct ScanResult {
    pub new: Vec<ArtifactRef>,
    pub modified: Vec<ArtifactRef>,
    pub revalidated: Vec<ArtifactRef>,
    pub invalid: Vec<InvalidArtifact>,
    pub malformed: Vec<MalformedArtifact>,
    pub unreadable: Vec<UnreadableArtifact>,
    pub partially_scanned_types: Vec<PartiallyScannedType>,
    pub removed: Vec<ArtifactRef>,
    pub unrecognized_dirs: Vec<String>,
}

#[derive(Debug)]
pub enum ScanError {
    Io(std::io::Error),
    Store(StoreError),
    WorkspaceMissing(PathBuf),
}

impl fmt::Display for ScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScanError::Io(err) => write!(f, "I/O error: {err}"),
            ScanError::Store(err) => write!(f, "{err}"),
            ScanError::WorkspaceMissing(path) => {
                write!(f, "workspace directory is missing: {}", path.display())
            }
        }
    }
}

impl std::error::Error for ScanError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ScanError::Io(err) => Some(err),
            ScanError::Store(err) => Some(err),
            ScanError::WorkspaceMissing(_) => None,
        }
    }
}

enum ScanDisposition {
    New,
    Modified,
    Revalidated,
    Unchanged,
}

#[derive(Default)]
struct TypeScanState {
    skipped_types: HashSet<String>,
    partially_scanned_types: HashMap<String, usize>,
}

pub fn scan(workspace_dir: &Path, store: &mut ArtifactStore) -> Result<ScanResult, ScanError> {
    let scan_timestamp_ms = current_time_ms();
    store.clear_scan_gaps();
    let known_types: HashSet<String> = store
        .artifact_type_names()
        .into_iter()
        .map(str::to_string)
        .collect();
    let existing_keys = store.all_instance_keys();
    let mut seen_keys = HashSet::new();
    let mut type_scan_state = TypeScanState::default();
    let mut result = ScanResult::default();

    if !workspace_dir.exists() {
        if existing_keys.is_empty() {
            return Ok(result);
        }
        return Err(ScanError::WorkspaceMissing(workspace_dir.to_path_buf()));
    }

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
            &mut type_scan_state,
            &mut result,
        )?;
    }

    result.unrecognized_dirs.sort();
    result
        .unreadable
        .sort_by(|left, right| left.path.cmp(&right.path));
    let mut partially_scanned_types: Vec<_> = type_scan_state
        .partially_scanned_types
        .into_iter()
        .map(|(artifact_type, unreadable_entries)| PartiallyScannedType {
            artifact_type,
            unreadable_entries,
        })
        .collect();
    partially_scanned_types.sort_by(|left, right| left.artifact_type.cmp(&right.artifact_type));
    result.partially_scanned_types = partially_scanned_types;

    for (artifact_type, instance_id) in existing_keys {
        if type_scan_state.skipped_types.contains(&artifact_type) {
            continue;
        }
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
    type_scan_state: &mut TypeScanState,
    result: &mut ScanResult,
) -> Result<(), ScanError> {
    let entries = match std::fs::read_dir(type_dir) {
        Ok(entries) => entries,
        Err(err) => {
            mark_type_partially_scanned(artifact_type, type_scan_state, store);
            result.unreadable.push(UnreadableArtifact {
                path: type_dir.to_path_buf(),
                error: err.to_string(),
            });
            return Ok(());
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                mark_type_partially_scanned(artifact_type, type_scan_state, store);
                result.unreadable.push(UnreadableArtifact {
                    path: type_dir.to_path_buf(),
                    error: err.to_string(),
                });
                continue;
            }
        };
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(err) => {
                mark_type_partially_scanned(artifact_type, type_scan_state, store);
                result.unreadable.push(UnreadableArtifact {
                    path,
                    error: err.to_string(),
                });
                continue;
            }
        };
        if !file_type.is_file() {
            continue;
        }

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
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) => {
                mark_instance_partially_scanned(
                    artifact_type,
                    &instance_id,
                    type_scan_state,
                    store,
                );
                result.unreadable.push(UnreadableArtifact {
                    path: path.clone(),
                    error: err.to_string(),
                });
                continue;
            }
        };
        let artifact = ArtifactInput {
            artifact_type,
            instance_id: &instance_id,
            path: &path,
        };

        match serde_json::from_slice::<Value>(&bytes) {
            Ok(data) => handle_json_artifact(
                artifact,
                &data,
                existing.as_ref(),
                store,
                scan_timestamp_ms,
                result,
            )?,
            Err(err) => handle_malformed_artifact(
                artifact,
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

fn mark_type_partially_scanned(
    artifact_type: &str,
    type_scan_state: &mut TypeScanState,
    store: &mut ArtifactStore,
) {
    type_scan_state
        .skipped_types
        .insert(artifact_type.to_string());
    *type_scan_state
        .partially_scanned_types
        .entry(artifact_type.to_string())
        .or_insert(0) += 1;
    store.mark_type_scan_gap(artifact_type);
}

fn mark_instance_partially_scanned(
    artifact_type: &str,
    instance_id: &str,
    type_scan_state: &mut TypeScanState,
    store: &mut ArtifactStore,
) {
    type_scan_state
        .skipped_types
        .insert(artifact_type.to_string());
    *type_scan_state
        .partially_scanned_types
        .entry(artifact_type.to_string())
        .or_insert(0) += 1;
    store.mark_instance_scan_gap(artifact_type, instance_id);
}

fn handle_json_artifact(
    artifact: ArtifactInput<'_>,
    data: &Value,
    existing: Option<&ArtifactState>,
    store: &mut ArtifactStore,
    scan_timestamp_ms: u64,
    result: &mut ScanResult,
) -> Result<(), ScanError> {
    let new_hash = content_hash(data);
    let current_schema_hash = store
        .schema_hash_for(artifact.artifact_type)
        .map_err(ScanError::Store)?;
    let disposition = classify_disposition(existing, &new_hash, &current_schema_hash);

    match disposition {
        ScanDisposition::New => {
            store
                .record_with_timestamp(
                    artifact.artifact_type,
                    artifact.instance_id,
                    artifact.path,
                    data,
                    scan_timestamp_ms,
                )
                .map_err(ScanError::Store)?;
            result.new.push(artifact.as_ref());
        }
        ScanDisposition::Modified => {
            store
                .record_with_timestamp(
                    artifact.artifact_type,
                    artifact.instance_id,
                    artifact.path,
                    data,
                    scan_timestamp_ms,
                )
                .map_err(ScanError::Store)?;
            result.modified.push(artifact.as_ref());
        }
        ScanDisposition::Revalidated => {
            let last_modified_ms = existing
                .expect("revalidated artifact must already exist")
                .last_modified_ms;
            store
                .record_with_timestamp(
                    artifact.artifact_type,
                    artifact.instance_id,
                    artifact.path,
                    data,
                    last_modified_ms,
                )
                .map_err(ScanError::Store)?;
            result.revalidated.push(artifact.as_ref());
        }
        ScanDisposition::Unchanged => {
            let existing_state = existing.expect("unchanged artifact must already exist");
            let data_work_unit = data
                .get("work_unit")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            if existing_state.work_unit.is_none() && data_work_unit.is_some() {
                // Backfill: pre-upgrade state lacks work_unit that the readable artifact provides.
                store
                    .record_with_timestamp(
                        artifact.artifact_type,
                        artifact.instance_id,
                        artifact.path,
                        data,
                        existing_state.last_modified_ms,
                    )
                    .map_err(ScanError::Store)?;
            } else if existing_state.path != artifact.path {
                store
                    .update_path(artifact.artifact_type, artifact.instance_id, artifact.path)
                    .map_err(ScanError::Store)?;
            }
        }
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
    existing: Option<&ArtifactState>,
    store: &mut ArtifactStore,
    scan_timestamp_ms: u64,
    result: &mut ScanResult,
) -> Result<(), ScanError> {
    let new_hash = raw_content_hash(bytes);
    let current_schema_hash = store
        .schema_hash_for(artifact.artifact_type)
        .map_err(ScanError::Store)?;
    let disposition = classify_disposition(existing, &new_hash, &current_schema_hash);

    let previous_work_unit = existing.and_then(|s| s.work_unit.clone());

    match disposition {
        ScanDisposition::New => {
            store
                .record_malformed_with_timestamp(
                    artifact.artifact_type,
                    artifact.instance_id,
                    artifact.path,
                    bytes,
                    error.clone(),
                    scan_timestamp_ms,
                    previous_work_unit,
                )
                .map_err(ScanError::Store)?;
            result.new.push(artifact.as_ref());
        }
        ScanDisposition::Modified => {
            store
                .record_malformed_with_timestamp(
                    artifact.artifact_type,
                    artifact.instance_id,
                    artifact.path,
                    bytes,
                    error.clone(),
                    scan_timestamp_ms,
                    previous_work_unit,
                )
                .map_err(ScanError::Store)?;
            result.modified.push(artifact.as_ref());
        }
        ScanDisposition::Revalidated => {
            let last_modified_ms = existing
                .expect("revalidated artifact must already exist")
                .last_modified_ms;
            store
                .record_malformed_with_timestamp(
                    artifact.artifact_type,
                    artifact.instance_id,
                    artifact.path,
                    bytes,
                    error.clone(),
                    last_modified_ms,
                    previous_work_unit,
                )
                .map_err(ScanError::Store)?;
            result.revalidated.push(artifact.as_ref());
        }
        ScanDisposition::Unchanged => {
            if existing.is_some_and(|state| state.path != artifact.path) {
                store
                    .update_path(artifact.artifact_type, artifact.instance_id, artifact.path)
                    .map_err(ScanError::Store)?;
            }
        }
    }

    result.malformed.push(MalformedArtifact {
        artifact_type: artifact.artifact_type.to_string(),
        instance_id: artifact.instance_id.to_string(),
        path: artifact.path.to_path_buf(),
        error,
    });

    Ok(())
}

fn classify_disposition(
    existing: Option<&ArtifactState>,
    new_hash: &str,
    current_schema_hash: &str,
) -> ScanDisposition {
    match existing {
        None => ScanDisposition::New,
        Some(state) if state.content_hash != new_hash => ScanDisposition::Modified,
        Some(state) if state.schema_hash != current_schema_hash => ScanDisposition::Revalidated,
        Some(_) => ScanDisposition::Unchanged,
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
    use crate::test_helpers::{make_artifact_type, make_store};
    use serde_json::json;
    use tempfile::TempDir;

    fn write_file(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn missing_workspace_is_empty_only_for_fresh_store() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        let mut store = make_store(&tmp.path().join("store"), vec!["report"]);

        let result = scan(&workspace, &mut store).unwrap();

        assert_eq!(result, ScanResult::default());
    }

    #[test]
    fn missing_workspace_with_existing_state_errors() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        let store_dir = tmp.path().join("store");
        let mut store = make_store(&store_dir, vec!["report"]);
        store
            .record(
                "report",
                "alpha",
                Path::new("report/alpha.json"),
                &json!({"title": "ok"}),
            )
            .unwrap();

        let err = scan(&workspace, &mut store).unwrap_err();

        assert!(matches!(err, ScanError::WorkspaceMissing(_)));
        assert!(store.get("report", "alpha").is_some());
        assert!(store_dir.join("report/alpha.json").exists());
    }

    #[test]
    fn schema_change_revalidates_unchanged_file() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        let artifact_path = workspace.join("report/alpha.json");
        write_file(&artifact_path, r#"{"title":"ok"}"#);

        let store_dir = tmp.path().join("store");
        let mut old_store = crate::store::ArtifactStore::new(
            vec![make_artifact_type(
                "report",
                json!({
                    "type": "object",
                    "required": ["title"],
                    "properties": { "title": { "type": "string" } }
                }),
            )],
            store_dir.clone(),
        )
        .unwrap();
        old_store
            .record_with_timestamp(
                "report",
                "alpha",
                &artifact_path,
                &json!({"title": "ok"}),
                1234,
            )
            .unwrap();

        let mut new_store = crate::store::ArtifactStore::new(
            vec![make_artifact_type(
                "report",
                json!({
                    "type": "object",
                    "required": ["title", "status"],
                    "properties": {
                        "title": { "type": "string" },
                        "status": { "type": "string" }
                    }
                }),
            )],
            store_dir,
        )
        .unwrap();

        let result = scan(&workspace, &mut new_store).unwrap();

        assert!(result.new.is_empty());
        assert!(result.modified.is_empty());
        assert_eq!(result.revalidated.len(), 1);
        assert_eq!(
            new_store.get("report", "alpha").unwrap().last_modified_ms,
            1234
        );
        assert!(matches!(
            new_store.get("report", "alpha").unwrap().status,
            ValidationStatus::Invalid(_)
        ));
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
        assert!(result.revalidated.is_empty());
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
        assert!(result.revalidated.is_empty());
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
        assert!(result.revalidated.is_empty());
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
        assert!(result.revalidated.is_empty());
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

    #[test]
    fn unchanged_artifact_refreshes_stored_path_when_workspace_moves() {
        let tmp = TempDir::new().unwrap();
        let old_workspace = tmp.path().join("old-workspace");
        let new_workspace = tmp.path().join("new-workspace");
        let old_path = old_workspace.join("report/item.json");
        let new_path = new_workspace.join("report/item.json");
        write_file(&new_path, r#"{"title":"ok"}"#);

        let store_dir = tmp.path().join("store");
        let mut store = make_store(&store_dir, vec!["report"]);
        store
            .record_with_timestamp("report", "item", &old_path, &json!({"title": "ok"}), 1234)
            .unwrap();

        let result = scan(&new_workspace, &mut store).unwrap();
        let state = store.get("report", "item").unwrap();

        assert_eq!(result, ScanResult::default());
        assert_eq!(state.path, new_path);
        assert_eq!(state.last_modified_ms, 1234);
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_file_is_reported_and_affects_all_work_units() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        let unreadable_path = workspace.join("report/item.json");
        let store_dir = tmp.path().join("store");
        let mut store = make_store(&store_dir, vec!["report"]);
        write_file(&unreadable_path, r#"{"title":"ok","work_unit":"wu-a"}"#);
        scan(&workspace, &mut store).unwrap();
        let original_state = store.get("report", "item").unwrap().clone();
        std::fs::set_permissions(&unreadable_path, std::fs::Permissions::from_mode(0o0)).unwrap();

        let result = scan(&workspace, &mut store).unwrap();
        let state = store.get("report", "item").unwrap();

        assert_eq!(result.unreadable.len(), 1);
        assert_eq!(result.unreadable[0].path, unreadable_path);
        assert_eq!(
            result.partially_scanned_types,
            vec![PartiallyScannedType {
                artifact_type: "report".to_string(),
                unreadable_entries: 1,
            }]
        );
        assert_eq!(state.path, original_state.path);
        assert_eq!(state.last_modified_ms, original_state.last_modified_ms);
        assert!(store.scan_gap_affects_work_unit("report", Some("wu-a")));
        assert!(store.scan_gap_affects_work_unit("report", Some("wu-b")));

        std::fs::set_permissions(&unreadable_path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn partially_scanned_type_suppresses_removals() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        let unreadable_path = workspace.join("report/item.json");
        write_file(&unreadable_path, r#"{"title":"ok"}"#);
        std::fs::set_permissions(&unreadable_path, std::fs::Permissions::from_mode(0o0)).unwrap();

        let store_dir = tmp.path().join("store");
        let mut store = make_store(&store_dir, vec!["report"]);
        store
            .record_with_timestamp(
                "report",
                "gone",
                &workspace.join("report/gone.json"),
                &json!({"title": "keep"}),
                1234,
            )
            .unwrap();

        let result = scan(&workspace, &mut store).unwrap();

        assert!(result.removed.is_empty());
        assert_eq!(
            result.partially_scanned_types,
            vec![PartiallyScannedType {
                artifact_type: "report".to_string(),
                unreadable_entries: 1,
            }]
        );
        assert!(store.get("report", "gone").is_some());
        assert!(store_dir.join("report/gone.json").exists());

        std::fs::set_permissions(&unreadable_path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    // --- Work unit backfill ---

    #[test]
    fn unchanged_artifact_backfills_work_unit() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        let artifact_path = workspace.join("report/item.json");
        write_file(&artifact_path, r#"{"title":"ok","work_unit":"wu-1"}"#);

        let store_dir = tmp.path().join("store");
        let mut store = make_store(&store_dir, vec!["report"]);
        // Simulate pre-upgrade state: record with same content but state has no work_unit.
        store
            .record_with_timestamp(
                "report",
                "item",
                &artifact_path,
                &json!({"title": "ok", "work_unit": "wu-1"}),
                1234,
            )
            .unwrap();
        // Manually clear work_unit to simulate pre-upgrade persisted state.
        // The easiest way: remove and re-insert with work_unit: None via a
        // direct record_inner call. Instead, we use a fresh store that loads
        // from disk, then patch the in-memory state + re-persist.
        {
            let state = store.get("report", "item").unwrap().clone();
            assert_eq!(state.work_unit, Some("wu-1".to_string()));
            // Overwrite the persisted file to strip work_unit.
            let mut patched = state;
            patched.work_unit = None;
            let json = serde_json::to_string_pretty(&patched).unwrap();
            std::fs::write(store_dir.join("report/item.json"), json).unwrap();
        }
        // Reload store from disk to pick up the patched state.
        let types = vec![make_artifact_type(
            "report",
            crate::test_helpers::simple_schema(),
        )];
        let mut store = ArtifactStore::new(types, store_dir).unwrap();
        assert_eq!(store.get("report", "item").unwrap().work_unit, None);

        let _result = scan(&workspace, &mut store).unwrap();

        assert_eq!(
            store.get("report", "item").unwrap().work_unit,
            Some("wu-1".to_string())
        );
    }

    #[test]
    fn unchanged_artifact_without_work_unit_stays_none() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        let artifact_path = workspace.join("report/item.json");
        write_file(&artifact_path, r#"{"title":"ok"}"#);

        let mut store = make_store(&tmp.path().join("store"), vec!["report"]);
        store
            .record_with_timestamp(
                "report",
                "item",
                &artifact_path,
                &json!({"title": "ok"}),
                1234,
            )
            .unwrap();
        assert_eq!(store.get("report", "item").unwrap().work_unit, None);

        let result = scan(&workspace, &mut store).unwrap();

        // No backfill needed — should remain Unchanged with no re-record.
        assert!(result.new.is_empty());
        assert!(result.modified.is_empty());
        assert!(result.revalidated.is_empty());
        assert_eq!(store.get("report", "item").unwrap().work_unit, None);
        assert_eq!(store.get("report", "item").unwrap().last_modified_ms, 1234);
    }

    #[test]
    fn unchanged_artifact_with_work_unit_not_re_recorded() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        let artifact_path = workspace.join("report/item.json");
        write_file(&artifact_path, r#"{"title":"ok","work_unit":"wu-1"}"#);

        let mut store = make_store(&tmp.path().join("store"), vec!["report"]);
        store
            .record_with_timestamp(
                "report",
                "item",
                &artifact_path,
                &json!({"title": "ok", "work_unit": "wu-1"}),
                1234,
            )
            .unwrap();
        assert_eq!(
            store.get("report", "item").unwrap().work_unit,
            Some("wu-1".to_string())
        );

        let result = scan(&workspace, &mut store).unwrap();

        // Already has work_unit — no re-record needed.
        assert!(result.new.is_empty());
        assert!(result.modified.is_empty());
        assert!(result.revalidated.is_empty());
        assert_eq!(store.get("report", "item").unwrap().last_modified_ms, 1234);
    }

    // --- Malformed work_unit preservation ---

    #[test]
    fn malformed_preserves_previous_work_unit() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        let artifact_path = workspace.join("report/item.json");

        // First, record a valid artifact with work_unit.
        write_file(&artifact_path, r#"{"title":"ok","work_unit":"wu-1"}"#);
        let mut store = make_store(&tmp.path().join("store"), vec!["report"]);
        scan(&workspace, &mut store).unwrap();
        assert_eq!(
            store.get("report", "item").unwrap().work_unit,
            Some("wu-1".to_string())
        );

        // Now make it malformed.
        write_file(&artifact_path, r#"{ nope }"#);
        scan(&workspace, &mut store).unwrap();

        let state = store.get("report", "item").unwrap();
        assert!(matches!(state.status, ValidationStatus::Malformed(_)));
        assert_eq!(state.work_unit, Some("wu-1".to_string()));
    }

    #[test]
    fn malformed_without_prior_state_gets_none() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        write_file(&workspace.join("report/item.json"), r#"{ nope }"#);
        let mut store = make_store(&tmp.path().join("store"), vec!["report"]);

        scan(&workspace, &mut store).unwrap();

        let state = store.get("report", "item").unwrap();
        assert!(matches!(state.status, ValidationStatus::Malformed(_)));
        assert_eq!(state.work_unit, None);
    }

    #[test]
    fn has_any_invalid_scoped_with_malformed() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");

        // Valid artifact in wu-a.
        write_file(
            &workspace.join("report/good.json"),
            r#"{"title":"ok","work_unit":"wu-a"}"#,
        );
        // Malformed artifact — will become wu-b after first recording valid then corrupting.
        write_file(
            &workspace.join("report/bad.json"),
            r#"{"title":"ok","work_unit":"wu-b"}"#,
        );

        let mut store = make_store(&tmp.path().join("store"), vec!["report"]);
        scan(&workspace, &mut store).unwrap();
        assert_eq!(
            store.get("report", "bad").unwrap().work_unit,
            Some("wu-b".to_string())
        );

        // Now corrupt the wu-b artifact.
        write_file(&workspace.join("report/bad.json"), r#"{ nope }"#);
        scan(&workspace, &mut store).unwrap();

        // Malformed in wu-b is NOT visible to wu-a query.
        assert!(!store.has_any_invalid("report", Some("wu-a")));
        // But IS visible to wu-b query.
        assert!(store.has_any_invalid("report", Some("wu-b")));
    }
}
