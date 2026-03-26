use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::model::ArtifactType;
use crate::validation::{ValidationError, Violation, validate_artifact};

/// Tracks artifact instances: their validation status, content hashes, and
/// filesystem paths. Persists state as JSON files in a store directory.
pub struct ArtifactStore {
    artifact_types: HashMap<String, ArtifactType>,
    artifacts: HashMap<(String, String), ArtifactState>,
    store_dir: PathBuf,
    // Populated by scan() for the current process only. These observations are
    // about scan completeness, not persisted artifact state.
    type_level_scan_gaps: HashSet<String>,
    instance_level_scan_gaps: HashSet<(String, String)>,
}

/// The recorded state of a single artifact instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactState {
    /// Filesystem path of the artifact source.
    pub path: PathBuf,
    /// Current validation status.
    pub status: ValidationStatus,
    /// Milliseconds since UNIX epoch when last recorded.
    pub last_modified_ms: u64,
    /// Content hash in the format `"sha256:<hex>"`.
    pub content_hash: String,
    /// Schema hash in the format `"sha256:<hex>"`.
    #[serde(default)]
    pub schema_hash: String,
    /// Work unit this artifact belongs to, extracted from artifact JSON at record time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_unit: Option<String>,
}

/// Validation status of an artifact instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStatus {
    /// The artifact conforms to its schema.
    Valid,
    /// The artifact violates its schema.
    Invalid(Vec<Violation>),
    /// The artifact file could not be parsed as JSON.
    Malformed(String),
    /// The artifact needs revalidation.
    Stale,
}

/// Errors that can occur during store operations.
#[derive(Debug)]
pub enum StoreError {
    /// The artifact type is not registered in this store.
    UnknownArtifactType(String),
    /// The artifact type's schema is malformed.
    InvalidSchema {
        artifact_type: String,
        detail: String,
    },
    /// Filesystem I/O failure.
    Io(std::io::Error),
    /// JSON serialization/deserialization failure.
    Serialization(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::UnknownArtifactType(name) => {
                write!(f, "unknown artifact type: '{name}'")
            }
            StoreError::InvalidSchema {
                artifact_type,
                detail,
            } => write!(
                f,
                "invalid schema for artifact type '{artifact_type}': {detail}"
            ),
            StoreError::Io(err) => write!(f, "I/O error: {err}"),
            StoreError::Serialization(detail) => {
                write!(f, "serialization error: {detail}")
            }
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StoreError::Io(err) => Some(err),
            _ => None,
        }
    }
}

/// Produce a deterministic JSON string by recursively sorting object keys.
fn canonical_json(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let entries: Vec<String> = keys
                .iter()
                .map(|k| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(*k).expect("string serialization"),
                        canonical_json(&map[*k])
                    )
                })
                .collect();
            format!("{{{}}}", entries.join(","))
        }
        Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(canonical_json).collect();
            format!("[{}]", items.join(","))
        }
        other => serde_json::to_string(other).expect("primitive serialization"),
    }
}

pub(crate) fn content_hash(value: &Value) -> String {
    let canonical = canonical_json(value);
    raw_content_hash(canonical.as_bytes())
}

pub(crate) fn schema_hash(value: &Value) -> String {
    content_hash(value)
}

pub(crate) fn raw_content_hash(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let result = hasher.finalize();
    format!("sha256:{result:x}")
}

fn matches_work_unit_filter(state_wu: &Option<String>, filter: Option<&str>) -> bool {
    match filter {
        None => true,
        Some(wu) => state_wu.as_deref().is_none_or(|s| s == wu),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct PersistedArtifactState {
    path: PersistedPath,
    display_path: String,
    status: ValidationStatus,
    last_modified_ms: u64,
    content_hash: String,
    #[serde(default)]
    schema_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    work_unit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
enum PersistedArtifactStateCompat {
    Current(PersistedArtifactState),
    Legacy(LegacyArtifactState),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct LegacyArtifactState {
    path: String,
    status: ValidationStatus,
    last_modified_ms: u64,
    content_hash: String,
    #[serde(default)]
    schema_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    work_unit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PersistedPath {
    UnixBytes(Vec<u8>),
    WindowsWide(Vec<u16>),
    Utf8(String),
}

impl PersistedArtifactState {
    fn from_state(state: &ArtifactState) -> Self {
        Self {
            path: PersistedPath::from_path(&state.path),
            display_path: display_path(&state.path),
            status: state.status.clone(),
            last_modified_ms: state.last_modified_ms,
            content_hash: state.content_hash.clone(),
            schema_hash: state.schema_hash.clone(),
            work_unit: state.work_unit.clone(),
        }
    }

    fn into_state(self) -> ArtifactState {
        ArtifactState {
            path: self.path.into_path_buf(),
            status: self.status,
            last_modified_ms: self.last_modified_ms,
            content_hash: self.content_hash,
            schema_hash: self.schema_hash,
            work_unit: self.work_unit,
        }
    }
}

impl LegacyArtifactState {
    fn into_state(self) -> ArtifactState {
        ArtifactState {
            path: PathBuf::from(self.path),
            status: self.status,
            last_modified_ms: self.last_modified_ms,
            content_hash: self.content_hash,
            schema_hash: self.schema_hash,
            work_unit: self.work_unit,
        }
    }
}

impl PersistedPath {
    fn from_path(path: &Path) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;

            return Self::UnixBytes(path.as_os_str().as_bytes().to_vec());
        }

        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;

            return Self::WindowsWide(path.as_os_str().encode_wide().collect());
        }

        #[allow(unreachable_code)]
        Self::Utf8(path.to_string_lossy().into_owned())
    }

    fn into_path_buf(self) -> PathBuf {
        match self {
            #[cfg(unix)]
            Self::UnixBytes(bytes) => {
                use std::ffi::OsString;
                use std::os::unix::ffi::OsStringExt;

                PathBuf::from(OsString::from_vec(bytes))
            }
            #[cfg(not(unix))]
            Self::UnixBytes(bytes) => PathBuf::from(String::from_utf8_lossy(&bytes).into_owned()),
            #[cfg(windows)]
            Self::WindowsWide(units) => {
                use std::ffi::OsString;
                use std::os::windows::ffi::OsStringExt;

                PathBuf::from(OsString::from_wide(&units))
            }
            #[cfg(not(windows))]
            Self::WindowsWide(units) => PathBuf::from(String::from_utf16_lossy(&units)),
            Self::Utf8(path) => PathBuf::from(path),
        }
    }
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn deserialize_artifact_state(content: &str) -> Result<ArtifactState, StoreError> {
    let state = serde_json::from_str::<PersistedArtifactStateCompat>(content)
        .map_err(|e| StoreError::Serialization(e.to_string()))?;

    Ok(match state {
        PersistedArtifactStateCompat::Current(current) => current.into_state(),
        PersistedArtifactStateCompat::Legacy(legacy) => legacy.into_state(),
    })
}

fn serialize_artifact_state(state: &ArtifactState) -> Result<String, StoreError> {
    serde_json::to_string_pretty(&PersistedArtifactState::from_state(state))
        .map_err(|e| StoreError::Serialization(e.to_string()))
}

impl ArtifactStore {
    /// Create a store, loading existing state from disk if present.
    /// Creates `store_dir` if it doesn't exist.
    pub fn new(artifact_types: Vec<ArtifactType>, store_dir: PathBuf) -> Result<Self, StoreError> {
        let type_map: HashMap<String, ArtifactType> = artifact_types
            .into_iter()
            .map(|at| (at.name.clone(), at))
            .collect();

        std::fs::create_dir_all(&store_dir).map_err(StoreError::Io)?;

        let mut artifacts = HashMap::new();

        for type_entry in std::fs::read_dir(&store_dir).map_err(StoreError::Io)? {
            let type_entry = type_entry.map_err(StoreError::Io)?;
            if !type_entry.file_type().map_err(StoreError::Io)?.is_dir() {
                continue;
            }
            let type_name = type_entry.file_name().to_string_lossy().into_owned();

            if !type_map.contains_key(&type_name) {
                tracing::warn!(
                    operation = "store_load",
                    outcome = "skipped_unknown_artifact_type",
                    artifact_type = %type_name,
                    "skipping unknown artifact type in store"
                );
                continue;
            }

            for inst_entry in std::fs::read_dir(type_entry.path()).map_err(StoreError::Io)? {
                let inst_entry = inst_entry.map_err(StoreError::Io)?;
                let path = inst_entry.path();
                if path.extension().is_some_and(|ext| ext == "json") {
                    let instance_id = path
                        .file_stem()
                        .expect("file has stem")
                        .to_string_lossy()
                        .into_owned();
                    let content = std::fs::read_to_string(&path).map_err(StoreError::Io)?;
                    let state = deserialize_artifact_state(&content)?;
                    artifacts.insert((type_name.clone(), instance_id), state);
                }
            }
        }

        Ok(Self {
            artifact_types: type_map,
            artifacts,
            store_dir,
            type_level_scan_gaps: HashSet::new(),
            instance_level_scan_gaps: HashSet::new(),
        })
    }

    /// Validate data against its schema and record the result.
    ///
    /// Both valid and invalid artifacts are stored — invalid state is
    /// meaningful for trigger evaluation. Errors only on infrastructure
    /// failures (unknown type, malformed schema, I/O).
    pub fn record(
        &mut self,
        artifact_type: &str,
        instance_id: &str,
        path: &Path,
        data: &Value,
    ) -> Result<(), StoreError> {
        self.record_with_timestamp(
            artifact_type,
            instance_id,
            path,
            data,
            crate::util::current_time_ms(),
        )
    }

    /// Record a malformed artifact file using a raw-byte content hash.
    pub fn record_malformed(
        &mut self,
        artifact_type: &str,
        instance_id: &str,
        path: &Path,
        raw_bytes: &[u8],
        error: impl Into<String>,
    ) -> Result<(), StoreError> {
        self.record_malformed_with_timestamp(
            artifact_type,
            instance_id,
            path,
            raw_bytes,
            error,
            crate::util::current_time_ms(),
            None,
        )
    }

    /// Returns current state of a specific instance.
    pub fn get(&self, artifact_type: &str, instance_id: &str) -> Option<&ArtifactState> {
        self.artifacts
            .get(&(artifact_type.to_string(), instance_id.to_string()))
    }

    /// True only if ALL matching instances of this type have Valid status.
    /// Returns false if no matching instances are recorded.
    ///
    /// When `work_unit` is `None`, all instances are considered.
    /// When `Some(wu)`, only instances belonging to that work unit
    /// (or unpartitioned instances with no work unit) are considered.
    pub fn is_valid(&self, artifact_type: &str, work_unit: Option<&str>) -> bool {
        let mut count = 0;
        for ((t, _), state) in &self.artifacts {
            if t == artifact_type && matches_work_unit_filter(&state.work_unit, work_unit) {
                if !matches!(state.status, ValidationStatus::Valid) {
                    return false;
                }
                count += 1;
            }
        }
        count > 0
    }

    /// Sets status to Stale for a specific instance. No-op if not recorded.
    pub fn invalidate(&mut self, artifact_type: &str, instance_id: &str) -> Result<(), StoreError> {
        let key = (artifact_type.to_string(), instance_id.to_string());
        if let Some(state) = self.artifacts.get(&key) {
            let mut snapshot = state.clone();
            snapshot.status = ValidationStatus::Stale;
            self.persist(artifact_type, instance_id, &snapshot)?;
            self.artifacts.get_mut(&key).unwrap().status = ValidationStatus::Stale;
        }
        Ok(())
    }

    /// Remove a specific recorded instance from memory and persisted state.
    pub fn remove(&mut self, artifact_type: &str, instance_id: &str) -> Result<(), StoreError> {
        let key = (artifact_type.to_string(), instance_id.to_string());
        if self.artifacts.remove(&key).is_none() {
            return Ok(());
        }

        let file_path = self
            .store_dir
            .join(artifact_type)
            .join(format!("{instance_id}.json"));
        match std::fs::remove_file(&file_path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(StoreError::Io(err)),
        }

        Ok(())
    }

    /// Refresh the stored filesystem path for an existing instance.
    pub fn update_path(
        &mut self,
        artifact_type: &str,
        instance_id: &str,
        path: &Path,
    ) -> Result<(), StoreError> {
        let key = (artifact_type.to_string(), instance_id.to_string());
        let Some(existing) = self.artifacts.get(&key).cloned() else {
            return Ok(());
        };

        if existing.path == path {
            return Ok(());
        }

        let mut updated = existing;
        updated.path = path.to_path_buf();
        self.persist(artifact_type, instance_id, &updated)?;
        self.artifacts.insert(key, updated);
        Ok(())
    }

    /// True if at least one matching instance of this type has `Invalid` or `Malformed` status.
    ///
    /// Scoping follows the same rules as [`is_valid`](Self::is_valid).
    pub fn has_any_invalid(&self, artifact_type: &str, work_unit: Option<&str>) -> bool {
        self.artifacts.iter().any(|((t, _), state)| {
            t == artifact_type
                && matches_work_unit_filter(&state.work_unit, work_unit)
                && matches!(
                    state.status,
                    ValidationStatus::Invalid(_) | ValidationStatus::Malformed(_)
                )
        })
    }

    /// Returns sorted names of all registered artifact types.
    pub fn artifact_type_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.artifact_types.keys().map(|s| s.as_str()).collect();
        names.sort();
        names
    }

    /// Returns `(instance_id, state)` pairs for a given artifact type,
    /// sorted by instance_id. Returns an empty vec if no matching
    /// instances exist.
    ///
    /// Scoping follows the same rules as [`is_valid`](Self::is_valid).
    pub fn instances_of(
        &self,
        artifact_type: &str,
        work_unit: Option<&str>,
    ) -> Vec<(&str, &ArtifactState)> {
        let mut pairs: Vec<(&str, &ArtifactState)> = self
            .artifacts
            .iter()
            .filter(|((t, _), state)| {
                t == artifact_type && matches_work_unit_filter(&state.work_unit, work_unit)
            })
            .map(|((_, id), state)| (id.as_str(), state))
            .collect();
        pairs.sort_by_key(|(id, _)| *id);
        pairs
    }

    /// Returns sorted `(artifact_type, instance_id)` keys for all recorded instances.
    pub fn all_instance_keys(&self) -> Vec<(String, String)> {
        let mut keys: Vec<(String, String)> = self.artifacts.keys().cloned().collect();
        keys.sort();
        keys
    }

    /// Returns the most recent `last_modified_ms` across matching instances of
    /// this type, or `None` if no matching instances are recorded.
    ///
    /// Scoping follows the same rules as [`is_valid`](Self::is_valid).
    pub fn latest_modification_ms(
        &self,
        artifact_type: &str,
        work_unit: Option<&str>,
    ) -> Option<u64> {
        self.artifacts
            .iter()
            .filter(|((t, _), state)| {
                t == artifact_type && matches_work_unit_filter(&state.work_unit, work_unit)
            })
            .map(|(_, state)| state.last_modified_ms)
            .max()
    }

    pub(crate) fn clear_scan_gaps(&mut self) {
        self.type_level_scan_gaps.clear();
        self.instance_level_scan_gaps.clear();
    }

    pub(crate) fn mark_type_scan_gap(&mut self, artifact_type: &str) {
        self.type_level_scan_gaps.insert(artifact_type.to_string());
    }

    pub(crate) fn mark_instance_scan_gap(&mut self, artifact_type: &str, instance_id: &str) {
        self.instance_level_scan_gaps
            .insert((artifact_type.to_string(), instance_id.to_string()));
    }

    pub(crate) fn has_any_scan_gap_for_type(&self, artifact_type: &str) -> bool {
        self.type_level_scan_gaps.contains(artifact_type)
            || self
                .instance_level_scan_gaps
                .iter()
                .any(|(gap_type, _)| gap_type == artifact_type)
    }

    pub(crate) fn scan_gap_affects_work_unit(
        &self,
        artifact_type: &str,
        _work_unit: Option<&str>,
    ) -> bool {
        self.type_level_scan_gaps.contains(artifact_type)
            || self
                .instance_level_scan_gaps
                .iter()
                .any(|(gap_type, _)| gap_type == artifact_type)
    }

    /// Look up a registered artifact type by name.
    pub fn artifact_type(&self, name: &str) -> Option<&ArtifactType> {
        self.artifact_types.get(name)
    }

    pub fn fork(&self, store_dir: PathBuf) -> Result<Self, StoreError> {
        std::fs::create_dir_all(&store_dir).map_err(StoreError::Io)?;

        Ok(Self {
            artifact_types: self.artifact_types.clone(),
            artifacts: self.artifacts.clone(),
            store_dir,
            type_level_scan_gaps: self.type_level_scan_gaps.clone(),
            instance_level_scan_gaps: self.instance_level_scan_gaps.clone(),
        })
    }

    pub fn record_with_timestamp(
        &mut self,
        artifact_type: &str,
        instance_id: &str,
        path: &Path,
        data: &Value,
        timestamp_ms: u64,
    ) -> Result<(), StoreError> {
        self.record_inner(
            artifact_type,
            instance_id,
            self.build_state_from_json(artifact_type, path, data, timestamp_ms)?,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_malformed_with_timestamp(
        &mut self,
        artifact_type: &str,
        instance_id: &str,
        path: &Path,
        raw_bytes: &[u8],
        error: impl Into<String>,
        timestamp_ms: u64,
        work_unit: Option<String>,
    ) -> Result<(), StoreError> {
        let schema_hash = self.schema_hash_for(artifact_type)?;
        let state = ArtifactState {
            path: path.to_path_buf(),
            status: ValidationStatus::Malformed(error.into()),
            last_modified_ms: timestamp_ms,
            content_hash: raw_content_hash(raw_bytes),
            schema_hash,
            work_unit,
        };
        self.record_inner(artifact_type, instance_id, state)
    }

    pub(crate) fn schema_hash_for(&self, artifact_type: &str) -> Result<String, StoreError> {
        let at = self
            .artifact_types
            .get(artifact_type)
            .ok_or_else(|| StoreError::UnknownArtifactType(artifact_type.to_string()))?;
        Ok(schema_hash(&at.schema))
    }

    fn build_state_from_json(
        &self,
        artifact_type: &str,
        path: &Path,
        data: &Value,
        timestamp_ms: u64,
    ) -> Result<ArtifactState, StoreError> {
        let at = self
            .artifact_types
            .get(artifact_type)
            .ok_or_else(|| StoreError::UnknownArtifactType(artifact_type.to_string()))?;

        let status = match validate_artifact(data, at) {
            Ok(()) => ValidationStatus::Valid,
            Err(ValidationError::InvalidArtifact { violations, .. }) => {
                ValidationStatus::Invalid(violations)
            }
            Err(ValidationError::InvalidSchema {
                artifact_type,
                detail,
            }) => {
                return Err(StoreError::InvalidSchema {
                    artifact_type,
                    detail,
                });
            }
        };

        let hash = content_hash(data);
        let work_unit = data
            .get("work_unit")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Ok(ArtifactState {
            path: path.to_path_buf(),
            status,
            last_modified_ms: timestamp_ms,
            content_hash: hash,
            schema_hash: schema_hash(&at.schema),
            work_unit,
        })
    }

    fn record_inner(
        &mut self,
        artifact_type: &str,
        instance_id: &str,
        state: ArtifactState,
    ) -> Result<(), StoreError> {
        self.persist(artifact_type, instance_id, &state)?;
        self.artifacts
            .insert((artifact_type.to_string(), instance_id.to_string()), state);

        Ok(())
    }

    fn persist(
        &self,
        artifact_type: &str,
        instance_id: &str,
        state: &ArtifactState,
    ) -> Result<(), StoreError> {
        let type_dir = self.store_dir.join(artifact_type);
        std::fs::create_dir_all(&type_dir).map_err(StoreError::Io)?;
        let file_path = type_dir.join(format!("{instance_id}.json"));
        let tmp_path = type_dir.join(format!("{instance_id}.json.tmp"));
        let json = serialize_artifact_state(state)?;
        std::fs::write(&tmp_path, json).map_err(StoreError::Io)?;
        std::fs::rename(&tmp_path, &file_path).map_err(StoreError::Io)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{make_artifact_type, make_store, simple_schema};
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn record_valid_artifact() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("artifacts"), vec!["report"]);

        let data = json!({"title": "Q1", "score": 95});
        let path = Path::new("reports/q1.json");
        store.record("report", "q1", path, &data).unwrap();

        let state = store.get("report", "q1").unwrap();
        assert_eq!(state.status, ValidationStatus::Valid);
        assert_eq!(state.path, path);
        assert!(state.content_hash.starts_with("sha256:"));
        assert_eq!(state.schema_hash, schema_hash(&simple_schema()));
        assert!(state.last_modified_ms > 0);
    }

    #[test]
    fn record_invalid_artifact() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("artifacts"), vec!["report"]);

        let data = json!({"score": 42}); // missing required "title"
        store
            .record("report", "bad", Path::new("r.json"), &data)
            .unwrap();

        let state = store.get("report", "bad").unwrap();
        match &state.status {
            ValidationStatus::Invalid(violations) => {
                assert!(!violations.is_empty());
            }
            other => panic!("expected Invalid, got: {other:?}"),
        }
    }

    #[test]
    fn record_malformed_artifact() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("artifacts"), vec!["report"]);

        store
            .record_malformed(
                "report",
                "bad",
                Path::new("bad.json"),
                br#"{ not json }"#,
                "expected value",
            )
            .unwrap();

        let state = store.get("report", "bad").unwrap();
        assert_eq!(
            state.status,
            ValidationStatus::Malformed("expected value".to_string())
        );
        assert_eq!(state.content_hash, raw_content_hash(br#"{ not json }"#));
        assert_eq!(state.schema_hash, schema_hash(&simple_schema()));
    }

    #[test]
    fn unknown_artifact_type_errors() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("artifacts"), vec!["report"]);

        let result = store.record("nonexistent", "x", Path::new("x.json"), &json!({}));
        assert!(matches!(
            result,
            Err(StoreError::UnknownArtifactType(ref name)) if name == "nonexistent"
        ));
    }

    #[test]
    fn is_valid_true_for_valid_false_for_invalid_malformed_stale_missing() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("artifacts"), vec!["report"]);

        // No instances recorded — false.
        assert!(!store.is_valid("report", None));

        // Record valid instance.
        store
            .record(
                "report",
                "good",
                Path::new("g.json"),
                &json!({"title": "ok"}),
            )
            .unwrap();
        assert!(store.is_valid("report", None));

        // Record invalid instance of a different type to avoid contamination.
        // Instead, create a second store with two types.
        let tmp2 = TempDir::new().unwrap();
        let mut store2 = ArtifactStore::new(
            vec![make_artifact_type("report", simple_schema())],
            tmp2.path().join("artifacts"),
        )
        .unwrap();

        // Invalid.
        store2
            .record("report", "bad", Path::new("b.json"), &json!({"score": 1}))
            .unwrap();
        assert!(!store2.is_valid("report", None));

        // Malformed.
        let tmp4 = TempDir::new().unwrap();
        let mut store4 = make_store(&tmp4.path().join("artifacts"), vec!["report"]);
        store4
            .record_malformed(
                "report",
                "m",
                Path::new("m.json"),
                b"not json",
                "expected value",
            )
            .unwrap();
        assert!(!store4.is_valid("report", None));

        // Stale.
        let tmp3 = TempDir::new().unwrap();
        let mut store3 = make_store(&tmp3.path().join("artifacts"), vec!["report"]);
        store3
            .record("report", "s", Path::new("s.json"), &json!({"title": "ok"}))
            .unwrap();
        store3.invalidate("report", "s").unwrap();
        assert!(!store3.is_valid("report", None));

        // Missing type entirely.
        assert!(!store.is_valid("nonexistent", None));
    }

    #[test]
    fn invalidate_marks_stale() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("artifacts"), vec!["report"]);

        store
            .record(
                "report",
                "r1",
                Path::new("r1.json"),
                &json!({"title": "ok"}),
            )
            .unwrap();
        assert_eq!(
            store.get("report", "r1").unwrap().status,
            ValidationStatus::Valid
        );

        store.invalidate("report", "r1").unwrap();
        assert_eq!(
            store.get("report", "r1").unwrap().status,
            ValidationStatus::Stale
        );
    }

    #[test]
    fn persistence_round_trip() {
        let tmp = TempDir::new().unwrap();
        let store_dir = tmp.path().join("artifacts");

        let types = vec![make_artifact_type("report", simple_schema())];
        let data = json!({"title": "persisted", "score": 10});
        let path = Path::new("reports/persisted.json");

        // Create store and record.
        {
            let mut store = ArtifactStore::new(types.clone(), store_dir.clone()).unwrap();
            store.record("report", "p1", path, &data).unwrap();
        }

        // Create new store from same directory.
        let store2 = ArtifactStore::new(types, store_dir).unwrap();
        let state = store2.get("report", "p1").unwrap();
        assert_eq!(state.status, ValidationStatus::Valid);
        assert_eq!(state.path, path);
        assert_eq!(state.content_hash, content_hash(&data));
        assert_eq!(state.schema_hash, schema_hash(&simple_schema()));
        assert!(state.last_modified_ms > 0);
    }

    #[test]
    fn canonical_json_deterministic() {
        // Nested objects — canonical_json must sort keys at every level.
        let val1 = json!({"z": 1, "a": {"y": 2, "x": 1}});
        let val2 = json!({"a": {"x": 1, "y": 2}, "z": 1});
        assert_eq!(canonical_json(&val1), canonical_json(&val2));

        // Verify the actual format: sorted keys, no whitespace.
        assert_eq!(canonical_json(&val1), r#"{"a":{"x":1,"y":2},"z":1}"#);
    }

    #[test]
    fn content_hash_changes_on_different_data() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("artifacts"), vec!["report"]);

        let data1 = json!({"title": "first"});
        let data2 = json!({"title": "second"});

        store
            .record("report", "r1", Path::new("r1.json"), &data1)
            .unwrap();
        store
            .record("report", "r2", Path::new("r2.json"), &data2)
            .unwrap();

        let hash1 = &store.get("report", "r1").unwrap().content_hash;
        let hash2 = &store.get("report", "r2").unwrap().content_hash;
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn invalidate_noop_on_missing() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("artifacts"), vec!["report"]);

        // Should not panic or error — returns Ok for unrecorded instances.
        store.invalidate("report", "nonexistent").unwrap();
        store.invalidate("nonexistent", "anything").unwrap();
    }

    #[test]
    fn multiple_instances_of_same_type() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("artifacts"), vec!["report"]);

        store
            .record(
                "report",
                "alpha",
                Path::new("a.json"),
                &json!({"title": "A"}),
            )
            .unwrap();
        store
            .record(
                "report",
                "beta",
                Path::new("b.json"),
                &json!({"title": "B"}),
            )
            .unwrap();

        let a = store.get("report", "alpha").unwrap();
        let b = store.get("report", "beta").unwrap();
        assert_eq!(a.path, Path::new("a.json"));
        assert_eq!(b.path, Path::new("b.json"));
        assert_ne!(a.content_hash, b.content_hash);
    }

    #[test]
    fn invalidate_persists_to_disk() {
        let tmp = TempDir::new().unwrap();
        let store_dir = tmp.path().join("artifacts");

        let types = vec![make_artifact_type("report", simple_schema())];

        // Record, then invalidate.
        {
            let mut store = ArtifactStore::new(types.clone(), store_dir.clone()).unwrap();
            store
                .record(
                    "report",
                    "r1",
                    Path::new("r1.json"),
                    &json!({"title": "ok"}),
                )
                .unwrap();
            store.invalidate("report", "r1").unwrap();
        }

        // Reload from disk — must see Stale, not Valid.
        let store2 = ArtifactStore::new(types, store_dir).unwrap();
        assert_eq!(
            store2.get("report", "r1").unwrap().status,
            ValidationStatus::Stale
        );
    }

    #[test]
    fn new_skips_unknown_artifact_types() {
        let tmp = TempDir::new().unwrap();
        let store_dir = tmp.path().join("artifacts");

        // Record with type "report".
        {
            let mut store = make_store(&store_dir, vec!["report"]);
            store
                .record(
                    "report",
                    "r1",
                    Path::new("r1.json"),
                    &json!({"title": "ok"}),
                )
                .unwrap();
        }

        // Reload with a different set of known types — "report" is now unknown.
        let other_types = vec![make_artifact_type("config", json!({"type": "object"}))];
        let store2 = ArtifactStore::new(other_types, store_dir).unwrap();
        assert!(store2.get("report", "r1").is_none());
    }

    #[test]
    fn is_valid_checks_all_instances() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("artifacts"), vec!["report"]);

        // One valid instance.
        store
            .record(
                "report",
                "good",
                Path::new("g.json"),
                &json!({"title": "ok"}),
            )
            .unwrap();
        assert!(store.is_valid("report", None));

        // Add one invalid instance — is_valid must now be false.
        store
            .record("report", "bad", Path::new("b.json"), &json!({"score": 1}))
            .unwrap();
        assert!(!store.is_valid("report", None));
    }

    #[test]
    fn has_any_invalid_true_with_invalid_instance() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("artifacts"), vec!["report"]);

        store
            .record("report", "bad", Path::new("b.json"), &json!({"score": 1}))
            .unwrap();
        assert!(store.has_any_invalid("report", None));
    }

    #[test]
    fn has_any_invalid_true_with_malformed_instance() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("artifacts"), vec!["report"]);

        store
            .record_malformed("report", "bad", Path::new("b.json"), b"not json", "oops")
            .unwrap();
        assert!(store.has_any_invalid("report", None));
    }

    #[test]
    fn has_any_invalid_false_when_all_valid() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("artifacts"), vec!["report"]);

        store
            .record(
                "report",
                "good",
                Path::new("g.json"),
                &json!({"title": "ok"}),
            )
            .unwrap();
        assert!(!store.has_any_invalid("report", None));
    }

    #[test]
    fn has_any_invalid_false_with_no_instances() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp.path().join("artifacts"), vec!["report"]);

        assert!(!store.has_any_invalid("report", None));
    }

    #[test]
    fn latest_modification_ms_returns_max() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("artifacts"), vec!["report"]);

        store
            .record_with_timestamp(
                "report",
                "old",
                Path::new("old.json"),
                &json!({"title": "old"}),
                1000,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "report",
                "new",
                Path::new("new.json"),
                &json!({"title": "new"}),
                2000,
            )
            .unwrap();
        assert_eq!(store.latest_modification_ms("report", None), Some(2000));
    }

    #[test]
    fn latest_modification_ms_none_for_missing_type() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp.path().join("artifacts"), vec!["report"]);

        assert_eq!(store.latest_modification_ms("report", None), None);
        assert_eq!(store.latest_modification_ms("nonexistent", None), None);
    }

    #[test]
    fn artifact_type_names_returns_sorted() {
        let tmp = TempDir::new().unwrap();
        let store = ArtifactStore::new(
            vec![
                make_artifact_type("zebra", json!({"type": "object"})),
                make_artifact_type("alpha", json!({"type": "object"})),
                make_artifact_type("middle", json!({"type": "object"})),
            ],
            tmp.path().join("artifacts"),
        )
        .unwrap();

        assert_eq!(
            store.artifact_type_names(),
            vec!["alpha", "middle", "zebra"]
        );
    }

    #[test]
    fn artifact_type_names_empty_store() {
        let tmp = TempDir::new().unwrap();
        let store = ArtifactStore::new(vec![], tmp.path().join("artifacts")).unwrap();

        let names: Vec<&str> = store.artifact_type_names();
        assert!(names.is_empty());
    }

    #[test]
    fn instances_of_returns_sorted_pairs() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("artifacts"), vec!["report"]);

        store
            .record(
                "report",
                "charlie",
                Path::new("c.json"),
                &json!({"title": "C"}),
            )
            .unwrap();
        store
            .record(
                "report",
                "alpha",
                Path::new("a.json"),
                &json!({"title": "A"}),
            )
            .unwrap();
        store
            .record(
                "report",
                "bravo",
                Path::new("b.json"),
                &json!({"title": "B"}),
            )
            .unwrap();

        let instances = store.instances_of("report", None);
        let ids: Vec<&str> = instances.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec!["alpha", "bravo", "charlie"]);

        // Verify state is accessible.
        assert_eq!(instances[0].1.path, Path::new("a.json"));
        assert_eq!(instances[1].1.path, Path::new("b.json"));
        assert_eq!(instances[2].1.path, Path::new("c.json"));
    }

    #[test]
    fn instances_of_unknown_type_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp.path().join("artifacts"), vec!["report"]);

        assert!(store.instances_of("nonexistent", None).is_empty());
    }

    #[test]
    fn instances_of_mixed_valid_invalid() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("artifacts"), vec!["report"]);

        // Valid instance.
        store
            .record(
                "report",
                "good",
                Path::new("g.json"),
                &json!({"title": "ok"}),
            )
            .unwrap();
        // Invalid instance (missing required "title").
        store
            .record("report", "bad", Path::new("b.json"), &json!({"score": 1}))
            .unwrap();

        let instances = store.instances_of("report", None);
        assert_eq!(instances.len(), 2);

        // Sorted: "bad" before "good".
        assert_eq!(instances[0].0, "bad");
        assert!(matches!(
            instances[0].1.status,
            ValidationStatus::Invalid(_)
        ));

        assert_eq!(instances[1].0, "good");
        assert_eq!(instances[1].1.status, ValidationStatus::Valid);
    }

    #[test]
    fn remove_deletes_in_memory_and_persisted_state() {
        let tmp = TempDir::new().unwrap();
        let store_dir = tmp.path().join("artifacts");
        let mut store = make_store(&store_dir, vec!["report"]);

        store
            .record(
                "report",
                "gone",
                Path::new("gone.json"),
                &json!({"title": "gone"}),
            )
            .unwrap();

        let persisted = store_dir.join("report/gone.json");
        assert!(persisted.is_file());

        store.remove("report", "gone").unwrap();

        assert!(store.get("report", "gone").is_none());
        assert!(!persisted.exists());
        assert!(!store.is_valid("report", None));
        assert!(!store.has_any_invalid("report", None));
        assert!(store.instances_of("report", None).is_empty());
    }

    #[test]
    fn all_instance_keys_returns_sorted_keys() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("artifacts"), vec!["a", "b"]);

        store
            .record("b", "two", Path::new("b2.json"), &json!({"title": "B"}))
            .unwrap();
        store
            .record("a", "one", Path::new("a1.json"), &json!({"title": "A"}))
            .unwrap();

        assert_eq!(
            store.all_instance_keys(),
            vec![
                ("a".to_string(), "one".to_string()),
                ("b".to_string(), "two".to_string())
            ]
        );
    }

    #[test]
    fn update_path_persists_new_path_without_other_changes() {
        let tmp = TempDir::new().unwrap();
        let store_dir = tmp.path().join("artifacts");
        let mut store = make_store(&store_dir, vec!["report"]);

        store
            .record(
                "report",
                "item",
                Path::new("old/item.json"),
                &json!({"title": "ok"}),
            )
            .unwrap();

        let before = store.get("report", "item").unwrap().clone();
        store
            .update_path("report", "item", Path::new("new/item.json"))
            .unwrap();

        let after = store.get("report", "item").unwrap();
        assert_eq!(after.path, Path::new("new/item.json"));
        assert_eq!(after.status, before.status);
        assert_eq!(after.content_hash, before.content_hash);
        assert_eq!(after.schema_hash, before.schema_hash);
        assert_eq!(after.last_modified_ms, before.last_modified_ms);

        let reloaded = make_store(&store_dir, vec!["report"]);
        assert_eq!(
            reloaded.get("report", "item").unwrap().path,
            Path::new("new/item.json")
        );
    }

    // --- Work unit scoping ---

    #[test]
    fn record_extracts_work_unit_from_json() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["report"]);

        store
            .record(
                "report",
                "r1",
                Path::new("r1.json"),
                &json!({"title": "ok", "work_unit": "wu-1"}),
            )
            .unwrap();

        assert_eq!(
            store.get("report", "r1").unwrap().work_unit,
            Some("wu-1".to_string())
        );
    }

    #[test]
    fn scan_gap_without_store_record_affects_all_scoped_work_units() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["report"]);

        store.mark_instance_scan_gap("report", "hidden");

        assert!(store.scan_gap_affects_work_unit("report", Some("wu-a")));
        assert!(store.scan_gap_affects_work_unit("report", Some("wu-b")));
    }

    #[test]
    fn scan_gap_with_store_record_affects_all_scoped_work_units() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["report"]);
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let known_path = workspace.join("known.json");
        std::fs::write(&known_path, r#"{"title":"ok","work_unit":"wu-a"}"#).unwrap();

        store
            .record(
                "report",
                "known",
                &known_path,
                &json!({"title": "ok", "work_unit": "wu-a"}),
            )
            .unwrap();
        store.mark_instance_scan_gap("report", "known");

        assert!(store.scan_gap_affects_work_unit("report", Some("wu-a")));
        assert!(store.scan_gap_affects_work_unit("report", Some("wu-b")));
    }

    #[test]
    fn record_sets_none_when_work_unit_absent() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["report"]);

        store
            .record(
                "report",
                "r1",
                Path::new("r1.json"),
                &json!({"title": "ok"}),
            )
            .unwrap();

        assert_eq!(store.get("report", "r1").unwrap().work_unit, None);
    }

    #[test]
    fn record_malformed_sets_none_work_unit() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["report"]);

        store
            .record_malformed("report", "bad", Path::new("b.json"), b"not json", "oops")
            .unwrap();

        assert_eq!(store.get("report", "bad").unwrap().work_unit, None);
    }

    #[test]
    fn work_unit_round_trips_through_persistence() {
        let tmp = TempDir::new().unwrap();
        let store_dir = tmp.path().join("s");
        let types = vec![make_artifact_type("report", simple_schema())];

        {
            let mut store = ArtifactStore::new(types.clone(), store_dir.clone()).unwrap();
            store
                .record(
                    "report",
                    "r1",
                    Path::new("r1.json"),
                    &json!({"title": "ok", "work_unit": "wu-1"}),
                )
                .unwrap();
        }

        let store2 = ArtifactStore::new(types, store_dir).unwrap();
        assert_eq!(
            store2.get("report", "r1").unwrap().work_unit,
            Some("wu-1".to_string())
        );
    }

    #[test]
    fn is_valid_scoped() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["report"]);

        // Valid in WU-A.
        store
            .record(
                "report",
                "a1",
                Path::new("a1.json"),
                &json!({"title": "ok", "work_unit": "wu-a"}),
            )
            .unwrap();
        // Invalid in WU-B.
        store
            .record(
                "report",
                "b1",
                Path::new("b1.json"),
                &json!({"score": 1, "work_unit": "wu-b"}),
            )
            .unwrap();

        // Scoped to WU-A: only sees the valid instance.
        assert!(store.is_valid("report", Some("wu-a")));
        // Scoped to WU-B: only sees the invalid instance.
        assert!(!store.is_valid("report", Some("wu-b")));
        // Unscoped: sees both, invalid blocks.
        assert!(!store.is_valid("report", None));
    }

    #[test]
    fn instances_of_scoped_includes_unpartitioned() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["report"]);

        // Unpartitioned instance (no work_unit).
        store
            .record(
                "report",
                "shared",
                Path::new("shared.json"),
                &json!({"title": "shared"}),
            )
            .unwrap();
        // WU-A instance.
        store
            .record(
                "report",
                "a1",
                Path::new("a1.json"),
                &json!({"title": "A", "work_unit": "wu-a"}),
            )
            .unwrap();

        let scoped = store.instances_of("report", Some("wu-a"));
        let ids: Vec<&str> = scoped.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec!["a1", "shared"]);
    }

    #[test]
    fn instances_of_scoped_excludes_other() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["report"]);

        store
            .record(
                "report",
                "a1",
                Path::new("a1.json"),
                &json!({"title": "A", "work_unit": "wu-a"}),
            )
            .unwrap();
        store
            .record(
                "report",
                "b1",
                Path::new("b1.json"),
                &json!({"title": "B", "work_unit": "wu-b"}),
            )
            .unwrap();

        let scoped = store.instances_of("report", Some("wu-b"));
        let ids: Vec<&str> = scoped.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec!["b1"]);
    }

    #[test]
    fn has_any_invalid_scoped() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["report"]);

        // Invalid in WU-A.
        store
            .record(
                "report",
                "bad",
                Path::new("bad.json"),
                &json!({"score": 1, "work_unit": "wu-a"}),
            )
            .unwrap();
        // Valid in WU-B.
        store
            .record(
                "report",
                "good",
                Path::new("good.json"),
                &json!({"title": "ok", "work_unit": "wu-b"}),
            )
            .unwrap();

        assert!(store.has_any_invalid("report", Some("wu-a")));
        assert!(!store.has_any_invalid("report", Some("wu-b")));
    }

    #[test]
    fn latest_modification_ms_scoped() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["report"]);

        store
            .record_with_timestamp(
                "report",
                "a1",
                Path::new("a1.json"),
                &json!({"title": "A", "work_unit": "wu-a"}),
                1000,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "report",
                "b1",
                Path::new("b1.json"),
                &json!({"title": "B", "work_unit": "wu-b"}),
                2000,
            )
            .unwrap();

        assert_eq!(
            store.latest_modification_ms("report", Some("wu-a")),
            Some(1000)
        );
        assert_eq!(
            store.latest_modification_ms("report", Some("wu-b")),
            Some(2000)
        );
        assert_eq!(store.latest_modification_ms("report", None), Some(2000));
    }

    #[test]
    fn malformed_preserves_previous_work_unit() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["report"]);

        // Record valid artifact with work_unit.
        store
            .record(
                "report",
                "r1",
                Path::new("r1.json"),
                &json!({"title": "ok", "work_unit": "wu-1"}),
            )
            .unwrap();
        assert_eq!(
            store.get("report", "r1").unwrap().work_unit,
            Some("wu-1".to_string())
        );

        // Now record it as malformed, passing previous work_unit.
        store
            .record_malformed_with_timestamp(
                "report",
                "r1",
                Path::new("r1.json"),
                b"{ nope }",
                "parse error",
                2000,
                Some("wu-1".to_string()),
            )
            .unwrap();

        let state = store.get("report", "r1").unwrap();
        assert!(matches!(state.status, ValidationStatus::Malformed(_)));
        assert_eq!(state.work_unit, Some("wu-1".to_string()));
    }

    #[test]
    fn malformed_without_prior_state_gets_none() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["report"]);

        store
            .record_malformed("report", "r1", Path::new("r1.json"), b"{ nope }", "oops")
            .unwrap();

        assert_eq!(store.get("report", "r1").unwrap().work_unit, None);
    }

    #[test]
    fn has_any_invalid_scoped_with_malformed() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["report"]);

        // Malformed artifact scoped to wu-b.
        store
            .record_malformed_with_timestamp(
                "report",
                "bad",
                Path::new("bad.json"),
                b"{ nope }",
                "oops",
                1000,
                Some("wu-b".to_string()),
            )
            .unwrap();

        // Not visible to wu-a.
        assert!(!store.has_any_invalid("report", Some("wu-a")));
        // Visible to wu-b.
        assert!(store.has_any_invalid("report", Some("wu-b")));
        // Visible unscoped.
        assert!(store.has_any_invalid("report", None));
    }

    #[cfg(unix)]
    #[test]
    fn record_persists_and_reloads_non_utf8_paths() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let tmp = TempDir::new().unwrap();
        let store_dir = tmp.path().join("artifacts");
        let mut store = make_store(&store_dir, vec!["report"]);
        let path = PathBuf::from(OsString::from_vec(b"reports/spec-\xFF.json".to_vec()));

        store
            .record("report", "spec", &path, &json!({"title": "Q1"}))
            .unwrap();
        assert_eq!(store.get("report", "spec").unwrap().path, path);

        let reloaded = ArtifactStore::new(
            vec![make_artifact_type("report", simple_schema())],
            store_dir,
        )
        .unwrap();
        assert_eq!(reloaded.get("report", "spec").unwrap().path, path);
    }

    #[cfg(unix)]
    #[test]
    fn loads_legacy_string_paths_and_rewrites_on_persist() {
        let tmp = TempDir::new().unwrap();
        let store_dir = tmp.path().join("artifacts");
        let type_dir = store_dir.join("report");
        std::fs::create_dir_all(&type_dir).unwrap();
        std::fs::write(
            type_dir.join("spec.json"),
            r#"{
  "path": "reports/spec.json",
  "status": "valid",
  "last_modified_ms": 1,
  "content_hash": "sha256:test",
  "schema_hash": "sha256:schema"
}"#,
        )
        .unwrap();

        let mut store = ArtifactStore::new(
            vec![make_artifact_type("report", simple_schema())],
            store_dir.clone(),
        )
        .unwrap();
        assert_eq!(
            store.get("report", "spec").unwrap().path,
            PathBuf::from("reports/spec.json")
        );

        store.invalidate("report", "spec").unwrap();
        let persisted = std::fs::read_to_string(type_dir.join("spec.json")).unwrap();
        assert!(persisted.contains("\"display_path\": \"reports/spec.json\""));
        assert!(persisted.contains("\"unix_bytes\""), "{persisted}");
    }
}
