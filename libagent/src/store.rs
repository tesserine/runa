use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::model::ArtifactType;
use crate::validation::{validate_artifact, ValidationError, Violation};

/// Tracks artifact instances: their validation status, content hashes, and
/// filesystem paths. Persists state as JSON files in a store directory.
pub struct ArtifactStore {
    artifact_types: HashMap<String, ArtifactType>,
    artifacts: HashMap<(String, String), ArtifactState>,
    store_dir: PathBuf,
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
}

/// Validation status of an artifact instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStatus {
    /// The artifact conforms to its schema.
    Valid,
    /// The artifact violates its schema.
    Invalid(Vec<Violation>),
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

fn content_hash(value: &Value) -> String {
    let canonical = canonical_json(value);
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    let result = hasher.finalize();
    format!("sha256:{result:x}")
}

impl ArtifactStore {
    /// Create a store, loading existing state from disk if present.
    /// Creates `store_dir` if it doesn't exist.
    pub fn new(
        artifact_types: Vec<ArtifactType>,
        store_dir: PathBuf,
    ) -> Result<Self, StoreError> {
        let type_map: HashMap<String, ArtifactType> = artifact_types
            .into_iter()
            .map(|at| (at.name.clone(), at))
            .collect();

        std::fs::create_dir_all(&store_dir).map_err(StoreError::Io)?;

        let mut artifacts = HashMap::new();

        for type_entry in std::fs::read_dir(&store_dir).map_err(StoreError::Io)? {
            let type_entry = type_entry.map_err(StoreError::Io)?;
            if !type_entry
                .file_type()
                .map_err(StoreError::Io)?
                .is_dir()
            {
                continue;
            }
            let type_name = type_entry.file_name().to_string_lossy().into_owned();

            for inst_entry in
                std::fs::read_dir(type_entry.path()).map_err(StoreError::Io)?
            {
                let inst_entry = inst_entry.map_err(StoreError::Io)?;
                let path = inst_entry.path();
                if path.extension().is_some_and(|ext| ext == "json") {
                    let instance_id = path
                        .file_stem()
                        .expect("file has stem")
                        .to_string_lossy()
                        .into_owned();
                    let content =
                        std::fs::read_to_string(&path).map_err(StoreError::Io)?;
                    let state: ArtifactState = serde_json::from_str(&content)
                        .map_err(|e| StoreError::Serialization(e.to_string()))?;
                    artifacts.insert((type_name.clone(), instance_id), state);
                }
            }
        }

        Ok(Self {
            artifact_types: type_map,
            artifacts,
            store_dir,
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
        let at = self.artifact_types.get(artifact_type).ok_or_else(|| {
            StoreError::UnknownArtifactType(artifact_type.to_string())
        })?;

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
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before UNIX epoch")
            .as_millis() as u64;

        let state = ArtifactState {
            path: path.to_path_buf(),
            status,
            last_modified_ms: now,
            content_hash: hash,
        };

        self.persist(artifact_type, instance_id, &state)?;
        self.artifacts.insert(
            (artifact_type.to_string(), instance_id.to_string()),
            state,
        );

        Ok(())
    }

    /// Returns current state of a specific instance.
    pub fn get(
        &self,
        artifact_type: &str,
        instance_id: &str,
    ) -> Option<&ArtifactState> {
        self.artifacts
            .get(&(artifact_type.to_string(), instance_id.to_string()))
    }

    /// True only if ALL instances of this type exist and have Valid status.
    /// Returns false if no instances are recorded for this type.
    pub fn is_valid(&self, artifact_type: &str) -> bool {
        let mut count = 0;
        for ((t, _), state) in &self.artifacts {
            if t == artifact_type {
                if !matches!(state.status, ValidationStatus::Valid) {
                    return false;
                }
                count += 1;
            }
        }
        count > 0
    }

    /// Sets status to Stale for a specific instance. No-op if not recorded.
    pub fn invalidate(&mut self, artifact_type: &str, instance_id: &str) {
        let key = (artifact_type.to_string(), instance_id.to_string());
        if let Some(state) = self.artifacts.get_mut(&key) {
            state.status = ValidationStatus::Stale;
            // Best-effort persist — in-memory state is authoritative during
            // the store's lifetime; persistence is for cross-session continuity.
            let snapshot = state.clone();
            let _ = self.persist(artifact_type, instance_id, &snapshot);
        }
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
        let json = serde_json::to_string_pretty(state)
            .map_err(|e| StoreError::Serialization(e.to_string()))?;
        std::fs::write(file_path, json).map_err(StoreError::Io)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn make_artifact_type(name: &str, schema: Value) -> ArtifactType {
        ArtifactType {
            name: name.into(),
            schema,
        }
    }

    fn simple_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "score": { "type": "integer" }
            },
            "required": ["title"]
        })
    }

    fn make_store(dir: &Path) -> ArtifactStore {
        ArtifactStore::new(
            vec![make_artifact_type("report", simple_schema())],
            dir.to_path_buf(),
        )
        .unwrap()
    }

    #[test]
    fn record_valid_artifact() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("artifacts"));

        let data = json!({"title": "Q1", "score": 95});
        let path = Path::new("reports/q1.json");
        store.record("report", "q1", path, &data).unwrap();

        let state = store.get("report", "q1").unwrap();
        assert_eq!(state.status, ValidationStatus::Valid);
        assert_eq!(state.path, path);
        assert!(state.content_hash.starts_with("sha256:"));
        assert!(state.last_modified_ms > 0);
    }

    #[test]
    fn record_invalid_artifact() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("artifacts"));

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
    fn unknown_artifact_type_errors() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("artifacts"));

        let result = store.record(
            "nonexistent",
            "x",
            Path::new("x.json"),
            &json!({}),
        );
        assert!(matches!(
            result,
            Err(StoreError::UnknownArtifactType(ref name)) if name == "nonexistent"
        ));
    }

    #[test]
    fn is_valid_true_for_valid_false_for_invalid_stale_missing() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("artifacts"));

        // No instances recorded — false.
        assert!(!store.is_valid("report"));

        // Record valid instance.
        store
            .record(
                "report",
                "good",
                Path::new("g.json"),
                &json!({"title": "ok"}),
            )
            .unwrap();
        assert!(store.is_valid("report"));

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
            .record(
                "report",
                "bad",
                Path::new("b.json"),
                &json!({"score": 1}),
            )
            .unwrap();
        assert!(!store2.is_valid("report"));

        // Stale.
        let tmp3 = TempDir::new().unwrap();
        let mut store3 = make_store(&tmp3.path().join("artifacts"));
        store3
            .record(
                "report",
                "s",
                Path::new("s.json"),
                &json!({"title": "ok"}),
            )
            .unwrap();
        store3.invalidate("report", "s");
        assert!(!store3.is_valid("report"));

        // Missing type entirely.
        assert!(!store.is_valid("nonexistent"));
    }

    #[test]
    fn invalidate_marks_stale() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("artifacts"));

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

        store.invalidate("report", "r1");
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
            let mut store =
                ArtifactStore::new(types.clone(), store_dir.clone()).unwrap();
            store.record("report", "p1", path, &data).unwrap();
        }

        // Create new store from same directory.
        let store2 = ArtifactStore::new(types, store_dir).unwrap();
        let state = store2.get("report", "p1").unwrap();
        assert_eq!(state.status, ValidationStatus::Valid);
        assert_eq!(state.path, path);
        assert_eq!(state.content_hash, content_hash(&data));
    }

    #[test]
    fn canonical_json_deterministic() {
        // Nested objects — canonical_json must sort keys at every level.
        let val1 = json!({"z": 1, "a": {"y": 2, "x": 1}});
        let val2 = json!({"a": {"x": 1, "y": 2}, "z": 1});
        assert_eq!(canonical_json(&val1), canonical_json(&val2));

        // Verify the actual format: sorted keys, no whitespace.
        assert_eq!(
            canonical_json(&val1),
            r#"{"a":{"x":1,"y":2},"z":1}"#
        );
    }

    #[test]
    fn content_hash_changes_on_different_data() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("artifacts"));

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
        let mut store = make_store(&tmp.path().join("artifacts"));

        // Should not panic or error.
        store.invalidate("report", "nonexistent");
        store.invalidate("nonexistent", "anything");
    }

    #[test]
    fn multiple_instances_of_same_type() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("artifacts"));

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
    fn is_valid_checks_all_instances() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("artifacts"));

        // One valid instance.
        store
            .record(
                "report",
                "good",
                Path::new("g.json"),
                &json!({"title": "ok"}),
            )
            .unwrap();
        assert!(store.is_valid("report"));

        // Add one invalid instance — is_valid must now be false.
        store
            .record(
                "report",
                "bad",
                Path::new("b.json"),
                &json!({"score": 1}),
            )
            .unwrap();
        assert!(!store.is_valid("report"));
    }
}
