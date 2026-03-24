//! Shared test utilities for libagent.
//!
//! Provides common helpers for constructing artifact types and stores
//! in unit tests. Only compiled under `#[cfg(test)]`.

use std::path::{Path, PathBuf};

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
