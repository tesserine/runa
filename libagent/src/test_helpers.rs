//! Shared test utilities for libagent.
//!
//! Provides common helpers for constructing artifact types and stores
//! in unit tests. Only compiled under `#[cfg(test)]`.

use std::path::Path;

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
