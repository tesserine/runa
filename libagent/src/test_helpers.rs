//! Shared test utilities for libagent.
//!
//! Provides common helpers for constructing artifact types and stores
//! in unit tests. Only compiled under `#[cfg(test)]`.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use serde_json::Value;

use crate::model::ArtifactType;
use crate::store::ArtifactStore;

/// Returns a minimal JSON Schema requiring an object with a `"title"` string.
pub fn simple_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "title": { "type": "string" }
        },
        "required": ["title"]
    })
}

/// Constructs an `ArtifactType` from a name and schema.
pub fn make_artifact_type(name: &str, schema: Value) -> ArtifactType {
    ArtifactType {
        name: name.into(),
        schema,
    }
}

/// Constructs an `ArtifactStore` with the given type names, each using
/// [`simple_schema`]. The store is created at `dir`.
pub fn make_store(dir: &Path, types: Vec<&str>) -> ArtifactStore {
    let artifact_types: Vec<ArtifactType> = types
        .into_iter()
        .map(|name| ArtifactType {
            name: name.into(),
            schema: simple_schema(),
        })
        .collect();
    ArtifactStore::new(artifact_types, dir.to_path_buf()).unwrap()
}

/// Write a methodology layout in `dir`: manifest TOML, schema files, and
/// protocol instruction files. Returns the manifest file path.
///
/// Duplicated in `runa-cli/tests/common/mod.rs` — CLI integration tests
/// cannot access this helper (`#[cfg(test)]` internal). Keep both in sync.
pub fn write_methodology(
    dir: &Path,
    manifest_toml: &str,
    schemas: &[(&str, &str)],
    protocols: &[&str],
) -> PathBuf {
    let manifest_path = dir.join("manifest.toml");
    std::fs::write(&manifest_path, manifest_toml).unwrap();

    let schemas_dir = dir.join("schemas");
    std::fs::create_dir_all(&schemas_dir).unwrap();
    for (name, content) in schemas {
        std::fs::write(schemas_dir.join(format!("{name}.schema.json")), content).unwrap();
    }

    for protocol_name in protocols {
        let protocol_dir = dir.join("protocols").join(protocol_name);
        std::fs::create_dir_all(&protocol_dir).unwrap();
        std::fs::write(
            protocol_dir.join("PROTOCOL.md"),
            format!("# {protocol_name}\n"),
        )
        .unwrap();
    }

    manifest_path
}

/// Serializes and restores process-wide environment changes in tests.
///
/// Rust runs tests in parallel by default, while environment variables are
/// process-global. Hold this guard for the full duration of any test that
/// mutates `std::env`.
pub struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl EnvGuard {
    pub fn set(values: &[(&'static str, &str)]) -> Self {
        Self::apply(&[], values)
    }

    pub fn unset(names: &[&'static str]) -> Self {
        Self::apply(names, &[])
    }

    pub fn unset_and_set(names: &[&'static str], values: &[(&'static str, &str)]) -> Self {
        Self::apply(names, values)
    }

    fn apply(names_to_unset: &[&'static str], values_to_set: &[(&'static str, &str)]) -> Self {
        let lock = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut names = names_to_unset.to_vec();
        for (name, _) in values_to_set {
            if !names.contains(name) {
                names.push(name);
            }
        }
        let previous = names
            .iter()
            .map(|name| (*name, std::env::var_os(name)))
            .collect::<Vec<_>>();
        for name in names_to_unset {
            unsafe { std::env::remove_var(name) };
        }
        for (name, value) in values_to_set {
            unsafe { std::env::set_var(name, value) };
        }
        Self {
            _lock: lock,
            previous,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (name, value) in &self.previous {
            match value {
                Some(value) => unsafe { std::env::set_var(name, value) },
                None => unsafe { std::env::remove_var(name) },
            }
        }
    }
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}
