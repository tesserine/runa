use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const ACTIVATIONS_FILENAME: &str = "activations.json";

/// Per-(protocol, work_unit) activation timestamp store.
///
/// Persists as `.runa/activations.json`. Each entry records the millisecond
/// timestamp of the most recent successful execution for a (protocol, work_unit)
/// pair.
pub struct ActivationStore {
    entries: HashMap<(String, Option<String>), u64>,
}

impl fmt::Debug for ActivationStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ActivationStore")
            .field("count", &self.entries.len())
            .finish()
    }
}

/// Errors that can occur during activation store operations.
#[derive(Debug)]
pub enum ActivationError {
    Io(std::io::Error),
    Parse(String),
}

impl fmt::Display for ActivationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ActivationError::Io(e) => write!(f, "activation store I/O error: {e}"),
            ActivationError::Parse(detail) => {
                write!(f, "activation store parse error: {detail}")
            }
        }
    }
}

impl std::error::Error for ActivationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ActivationError::Io(e) => Some(e),
            _ => None,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct ActivationsFile {
    entries: Vec<ActivationEntry>,
}

#[derive(Serialize, Deserialize)]
struct ActivationEntry {
    protocol: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    work_unit: Option<String>,
    timestamp_ms: u64,
}

impl ActivationStore {
    /// Load activation timestamps from `.runa/activations.json`.
    ///
    /// Returns an empty store if the file does not exist.
    pub fn load(runa_dir: &Path) -> Result<Self, ActivationError> {
        let path = runa_dir.join(ACTIVATIONS_FILENAME);
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self {
                    entries: HashMap::new(),
                });
            }
            Err(e) => return Err(ActivationError::Io(e)),
        };

        let file: ActivationsFile =
            serde_json::from_str(&content).map_err(|e| ActivationError::Parse(e.to_string()))?;

        let mut entries = HashMap::new();
        for entry in file.entries {
            entries.insert((entry.protocol, entry.work_unit), entry.timestamp_ms);
        }

        Ok(Self { entries })
    }

    /// Persist activation timestamps to `.runa/activations.json`.
    ///
    /// Uses atomic write (tmp + rename) matching the store.rs pattern.
    pub fn save(&self, runa_dir: &Path) -> Result<(), ActivationError> {
        let path = runa_dir.join(ACTIVATIONS_FILENAME);
        let tmp_path = runa_dir.join(format!("{ACTIVATIONS_FILENAME}.tmp"));

        let mut entries: Vec<ActivationEntry> = self
            .entries
            .iter()
            .map(|((protocol, work_unit), &timestamp_ms)| ActivationEntry {
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

        let file = ActivationsFile { entries };
        let json = serde_json::to_string_pretty(&file)
            .map_err(|e| ActivationError::Parse(e.to_string()))?;

        std::fs::write(&tmp_path, &json).map_err(ActivationError::Io)?;
        std::fs::rename(&tmp_path, &path).map_err(ActivationError::Io)?;
        Ok(())
    }

    /// Record an activation for (protocol, work_unit) at the current time.
    pub fn record(&mut self, protocol: &str, work_unit: Option<&str>) {
        self.record_at(protocol, work_unit, current_time_ms());
    }

    /// Get the activation timestamp for a specific (protocol, work_unit) pair.
    pub fn get(&self, protocol: &str, work_unit: Option<&str>) -> Option<u64> {
        self.entries
            .get(&(protocol.to_string(), work_unit.map(|s| s.to_string())))
            .copied()
    }

    /// Whether a (protocol, work_unit) pair has been activated at least once.
    pub fn is_activated(&self, protocol: &str, work_unit: Option<&str>) -> bool {
        self.get(protocol, work_unit).is_some()
    }

    /// Build a `HashMap<String, u64>` for `TriggerContext.activation_timestamps`,
    /// scoped to a work_unit.
    ///
    /// Returns all activation entries matching the given work_unit. Key is the
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
        let store = ActivationStore::load(tmp.path()).unwrap();
        assert!(store.entries.is_empty());
    }

    #[test]
    fn record_and_get() {
        let mut store = ActivationStore {
            entries: HashMap::new(),
        };
        store.record_at("ground", Some("feature-x"), 1000);
        assert_eq!(store.get("ground", Some("feature-x")), Some(1000));
        assert_eq!(store.get("ground", None), None);
        assert_eq!(store.get("other", Some("feature-x")), None);
    }

    #[test]
    fn record_overwrites_previous() {
        let mut store = ActivationStore {
            entries: HashMap::new(),
        };
        store.record_at("ground", Some("x"), 1000);
        store.record_at("ground", Some("x"), 2000);
        assert_eq!(store.get("ground", Some("x")), Some(2000));
    }

    #[test]
    fn is_activated() {
        let mut store = ActivationStore {
            entries: HashMap::new(),
        };
        assert!(!store.is_activated("ground", None));
        store.record_at("ground", None, 1000);
        assert!(store.is_activated("ground", None));
    }

    #[test]
    fn none_work_unit() {
        let mut store = ActivationStore {
            entries: HashMap::new(),
        };
        store.record_at("ground", None, 500);
        assert_eq!(store.get("ground", None), Some(500));
        assert!(store.is_activated("ground", None));
        assert!(!store.is_activated("ground", Some("x")));
    }

    #[test]
    fn save_and_load_round_trip() {
        let tmp = TempDir::new().unwrap();
        let mut store = ActivationStore {
            entries: HashMap::new(),
        };
        store.record_at("implement", Some("feature-x"), 1710000000000);
        store.record_at("ground", None, 1710000001000);
        store.save(tmp.path()).unwrap();

        let loaded = ActivationStore::load(tmp.path()).unwrap();
        assert_eq!(
            loaded.get("implement", Some("feature-x")),
            Some(1710000000000)
        );
        assert_eq!(loaded.get("ground", None), Some(1710000001000));
    }

    #[test]
    fn save_is_deterministic() {
        let tmp = TempDir::new().unwrap();
        let mut store = ActivationStore {
            entries: HashMap::new(),
        };
        store.record_at("b-proto", Some("wu-2"), 2000);
        store.record_at("a-proto", Some("wu-1"), 1000);
        store.record_at("a-proto", None, 500);
        store.save(tmp.path()).unwrap();

        let content = std::fs::read_to_string(tmp.path().join(ACTIVATIONS_FILENAME)).unwrap();
        let file: ActivationsFile = serde_json::from_str(&content).unwrap();
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
        std::fs::write(tmp.path().join(ACTIVATIONS_FILENAME), "not json").unwrap();
        let err = ActivationStore::load(tmp.path()).unwrap_err();
        assert!(matches!(err, ActivationError::Parse(_)));
    }

    #[test]
    fn timestamps_for_trigger_context_scoped() {
        let mut store = ActivationStore {
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
        let mut store = ActivationStore {
            entries: HashMap::new(),
        };
        let before = current_time_ms();
        store.record("test", Some("wu"));
        let after = current_time_ms();

        let ts = store.get("test", Some("wu")).unwrap();
        assert!(ts >= before && ts <= after);
    }
}
