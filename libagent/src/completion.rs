use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const COMPLETIONS_FILENAME: &str = "completions.json";
const LEGACY_FILENAME: &str = "activations.json";

/// Per-(protocol, work_unit) completion timestamp store.
///
/// Persists as `.runa/completions.json`. Each entry records the millisecond
/// timestamp of the most recent successful execution for a (protocol, work_unit)
/// pair.
pub struct CompletionStore {
    entries: HashMap<(String, Option<String>), u64>,
}

impl fmt::Debug for CompletionStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompletionStore")
            .field("count", &self.entries.len())
            .finish()
    }
}

/// Errors that can occur during completion store operations.
#[derive(Debug)]
pub enum CompletionError {
    Io(std::io::Error),
    Parse(String),
}

impl fmt::Display for CompletionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompletionError::Io(e) => write!(f, "completion store I/O error: {e}"),
            CompletionError::Parse(detail) => {
                write!(f, "completion store parse error: {detail}")
            }
        }
    }
}

impl std::error::Error for CompletionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CompletionError::Io(e) => Some(e),
            _ => None,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct CompletionsFile {
    entries: Vec<CompletionEntry>,
}

#[derive(Serialize, Deserialize)]
struct CompletionEntry {
    protocol: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    work_unit: Option<String>,
    timestamp_ms: u64,
}

impl CompletionStore {
    /// Load completion timestamps from `.runa/completions.json`.
    ///
    /// Falls back to `.runa/activations.json` for migration from older versions.
    /// Returns an empty store if neither file exists.
    pub fn load(runa_dir: &Path) -> Result<Self, CompletionError> {
        let path = runa_dir.join(COMPLETIONS_FILENAME);
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Migration: try legacy filename.
                let legacy = runa_dir.join(LEGACY_FILENAME);
                match std::fs::read_to_string(&legacy) {
                    Ok(c) => c,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        return Ok(Self {
                            entries: HashMap::new(),
                        });
                    }
                    Err(e) => return Err(CompletionError::Io(e)),
                }
            }
            Err(e) => return Err(CompletionError::Io(e)),
        };

        let file: CompletionsFile =
            serde_json::from_str(&content).map_err(|e| CompletionError::Parse(e.to_string()))?;

        let mut entries = HashMap::new();
        for entry in file.entries {
            entries.insert((entry.protocol, entry.work_unit), entry.timestamp_ms);
        }

        Ok(Self { entries })
    }

    /// Persist completion timestamps to `.runa/completions.json`.
    ///
    /// Uses atomic write (tmp + rename) matching the store.rs pattern.
    /// Removes the legacy `activations.json` if present (one-time migration).
    pub fn save(&self, runa_dir: &Path) -> Result<(), CompletionError> {
        let path = runa_dir.join(COMPLETIONS_FILENAME);
        let tmp_path = runa_dir.join(format!("{COMPLETIONS_FILENAME}.tmp"));

        let mut entries: Vec<CompletionEntry> = self
            .entries
            .iter()
            .map(|((protocol, work_unit), &timestamp_ms)| CompletionEntry {
                protocol: protocol.clone(),
                work_unit: work_unit.clone(),
                timestamp_ms,
            })
            .collect();
        // Deterministic output: sort by protocol then work_unit.
        entries.sort_by(|a, b| {
            a.protocol
                .cmp(&b.protocol)
                .then_with(|| a.work_unit.cmp(&b.work_unit))
        });

        let file = CompletionsFile { entries };
        let json = serde_json::to_string_pretty(&file)
            .map_err(|e| CompletionError::Parse(e.to_string()))?;

        std::fs::write(&tmp_path, &json).map_err(CompletionError::Io)?;
        std::fs::rename(&tmp_path, &path).map_err(CompletionError::Io)?;

        // Clean up legacy file if present.
        let legacy = runa_dir.join(LEGACY_FILENAME);
        if legacy.exists() {
            let _ = std::fs::remove_file(&legacy);
        }

        Ok(())
    }

    /// Record a completion for (protocol, work_unit) at the current time.
    pub fn record(&mut self, protocol: &str, work_unit: Option<&str>) {
        self.record_at(protocol, work_unit, current_time_ms());
    }

    /// Get the completion timestamp for a specific (protocol, work_unit) pair.
    pub fn get(&self, protocol: &str, work_unit: Option<&str>) -> Option<u64> {
        self.entries
            .get(&(protocol.to_string(), work_unit.map(|s| s.to_string())))
            .copied()
    }

    /// Whether a (protocol, work_unit) pair has completed at least once.
    pub fn is_completed(&self, protocol: &str, work_unit: Option<&str>) -> bool {
        self.get(protocol, work_unit).is_some()
    }

    /// Build a `HashMap<String, u64>` for `TriggerContext.completion_timestamps`,
    /// scoped to a work_unit.
    ///
    /// Returns all completion entries matching the given work_unit. Key is the
    /// protocol name, value is the timestamp. This bridges the per-(protocol,
    /// work_unit) storage to the existing trigger interface which expects
    /// per-protocol timestamps.
    pub fn timestamps_for_trigger_context(&self, work_unit: Option<&str>) -> HashMap<String, u64> {
        self.entries
            .iter()
            .filter(|((_, wu), _)| match (wu.as_deref(), work_unit) {
                (None, None) => true,
                (Some(a), Some(b)) => a == b,
                _ => false,
            })
            .map(|((protocol, _), &ts)| (protocol.clone(), ts))
            .collect()
    }

    pub(crate) fn record_at(&mut self, protocol: &str, work_unit: Option<&str>, timestamp_ms: u64) {
        self.entries.insert(
            (protocol.to_string(), work_unit.map(|s| s.to_string())),
            timestamp_ms,
        );
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
    use tempfile::TempDir;

    #[test]
    fn load_returns_empty_when_file_missing() {
        let tmp = TempDir::new().unwrap();
        let store = CompletionStore::load(tmp.path()).unwrap();
        assert!(store.entries.is_empty());
    }

    #[test]
    fn record_and_get() {
        let mut store = CompletionStore {
            entries: HashMap::new(),
        };
        store.record_at("ground", Some("feature-x"), 1000);
        assert_eq!(store.get("ground", Some("feature-x")), Some(1000));
        assert_eq!(store.get("ground", None), None);
        assert_eq!(store.get("other", Some("feature-x")), None);
    }

    #[test]
    fn record_overwrites_previous() {
        let mut store = CompletionStore {
            entries: HashMap::new(),
        };
        store.record_at("ground", Some("x"), 1000);
        store.record_at("ground", Some("x"), 2000);
        assert_eq!(store.get("ground", Some("x")), Some(2000));
    }

    #[test]
    fn is_completed() {
        let mut store = CompletionStore {
            entries: HashMap::new(),
        };
        assert!(!store.is_completed("ground", None));
        store.record_at("ground", None, 1000);
        assert!(store.is_completed("ground", None));
    }

    #[test]
    fn none_work_unit() {
        let mut store = CompletionStore {
            entries: HashMap::new(),
        };
        store.record_at("ground", None, 500);
        assert_eq!(store.get("ground", None), Some(500));
        assert!(store.is_completed("ground", None));
        assert!(!store.is_completed("ground", Some("x")));
    }

    #[test]
    fn save_and_load_round_trip() {
        let tmp = TempDir::new().unwrap();
        let mut store = CompletionStore {
            entries: HashMap::new(),
        };
        store.record_at("implement", Some("feature-x"), 1710000000000);
        store.record_at("ground", None, 1710000001000);
        store.save(tmp.path()).unwrap();

        let loaded = CompletionStore::load(tmp.path()).unwrap();
        assert_eq!(
            loaded.get("implement", Some("feature-x")),
            Some(1710000000000)
        );
        assert_eq!(loaded.get("ground", None), Some(1710000001000));
    }

    #[test]
    fn save_is_deterministic() {
        let tmp = TempDir::new().unwrap();
        let mut store = CompletionStore {
            entries: HashMap::new(),
        };
        store.record_at("b-proto", Some("wu-2"), 2000);
        store.record_at("a-proto", Some("wu-1"), 1000);
        store.record_at("a-proto", None, 500);
        store.save(tmp.path()).unwrap();

        let content = std::fs::read_to_string(tmp.path().join(COMPLETIONS_FILENAME)).unwrap();
        let file: CompletionsFile = serde_json::from_str(&content).unwrap();
        let names: Vec<(&str, Option<&str>)> = file
            .entries
            .iter()
            .map(|e| (e.protocol.as_str(), e.work_unit.as_deref()))
            .collect();
        // Sorted by protocol then work_unit (None < Some).
        assert_eq!(
            names,
            vec![
                ("a-proto", None),
                ("a-proto", Some("wu-1")),
                ("b-proto", Some("wu-2")),
            ]
        );
    }

    #[test]
    fn load_parse_error() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(COMPLETIONS_FILENAME), "not json").unwrap();
        let err = CompletionStore::load(tmp.path()).unwrap_err();
        assert!(matches!(err, CompletionError::Parse(_)));
    }

    #[test]
    fn timestamps_for_trigger_context_scoped() {
        let mut store = CompletionStore {
            entries: HashMap::new(),
        };
        store.record_at("ground", Some("feature-x"), 1000);
        store.record_at("implement", Some("feature-x"), 2000);
        store.record_at("ground", Some("feature-y"), 3000);
        store.record_at("deploy", None, 4000);

        let ctx = store.timestamps_for_trigger_context(Some("feature-x"));
        assert_eq!(ctx.len(), 2);
        assert_eq!(ctx["ground"], 1000);
        assert_eq!(ctx["implement"], 2000);

        let ctx_none = store.timestamps_for_trigger_context(None);
        assert_eq!(ctx_none.len(), 1);
        assert_eq!(ctx_none["deploy"], 4000);
    }

    #[test]
    fn record_uses_current_time() {
        let mut store = CompletionStore {
            entries: HashMap::new(),
        };
        let before = current_time_ms();
        store.record("test", Some("wu"));
        let after = current_time_ms();

        let ts = store.get("test", Some("wu")).unwrap();
        assert!(ts >= before && ts <= after);
    }

    #[test]
    fn load_migrates_from_legacy_filename() {
        let tmp = TempDir::new().unwrap();
        // Write to legacy filename.
        let mut store = CompletionStore {
            entries: HashMap::new(),
        };
        store.record_at("ground", None, 1000);

        let legacy_path = tmp.path().join(LEGACY_FILENAME);
        let file = CompletionsFile {
            entries: vec![CompletionEntry {
                protocol: "ground".into(),
                work_unit: None,
                timestamp_ms: 1000,
            }],
        };
        let json = serde_json::to_string_pretty(&file).unwrap();
        std::fs::write(&legacy_path, &json).unwrap();

        // Load should find legacy file.
        let loaded = CompletionStore::load(tmp.path()).unwrap();
        assert_eq!(loaded.get("ground", None), Some(1000));

        // Save should write to new filename and remove legacy.
        loaded.save(tmp.path()).unwrap();
        assert!(tmp.path().join(COMPLETIONS_FILENAME).exists());
        assert!(!legacy_path.exists());
    }

    #[test]
    fn load_prefers_new_filename_over_legacy() {
        let tmp = TempDir::new().unwrap();

        // Write both files with different data.
        let new_file = CompletionsFile {
            entries: vec![CompletionEntry {
                protocol: "new".into(),
                work_unit: None,
                timestamp_ms: 2000,
            }],
        };
        std::fs::write(
            tmp.path().join(COMPLETIONS_FILENAME),
            serde_json::to_string_pretty(&new_file).unwrap(),
        )
        .unwrap();

        let legacy_file = CompletionsFile {
            entries: vec![CompletionEntry {
                protocol: "old".into(),
                work_unit: None,
                timestamp_ms: 1000,
            }],
        };
        std::fs::write(
            tmp.path().join(LEGACY_FILENAME),
            serde_json::to_string_pretty(&legacy_file).unwrap(),
        )
        .unwrap();

        let loaded = CompletionStore::load(tmp.path()).unwrap();
        // Should load from new file.
        assert_eq!(loaded.get("new", None), Some(2000));
        assert_eq!(loaded.get("old", None), None);
    }
}
