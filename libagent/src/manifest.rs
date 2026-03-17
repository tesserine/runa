use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::model::{Manifest, TriggerCondition, is_valid_signal_name};

/// Errors that can occur when parsing a manifest file.
#[derive(Debug)]
pub enum ManifestError {
    /// Failed to read the manifest file.
    Io(std::io::Error),
    /// Failed to parse TOML or map it to the manifest structure.
    Parse(toml::de::Error),
    /// Two artifact types share the same name.
    DuplicateArtifactType(String),
    /// Two protocols share the same name.
    DuplicateProtocolName(String),
    /// An on_signal trigger uses an invalid signal name.
    InvalidSignalName(String),
    /// A schema file path reference does not exist on disk.
    SchemaFileNotFound {
        artifact_type: String,
        path: PathBuf,
    },
    /// A schema file exists but contains invalid JSON.
    SchemaFileInvalidJson {
        artifact_type: String,
        path: PathBuf,
        detail: String,
    },
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestError::Io(e) => write!(f, "failed to read manifest: {e}"),
            ManifestError::Parse(e) => write!(f, "invalid manifest: {e}"),
            ManifestError::DuplicateArtifactType(name) => {
                write!(f, "duplicate artifact type name: {name}")
            }
            ManifestError::DuplicateProtocolName(name) => {
                write!(f, "duplicate protocol name: {name}")
            }
            ManifestError::InvalidSignalName(name) => {
                write!(
                    f,
                    "invalid signal name '{name}': expected pattern [a-z0-9][a-z0-9_-]*"
                )
            }
            ManifestError::SchemaFileNotFound {
                artifact_type,
                path,
            } => {
                write!(
                    f,
                    "schema file not found for artifact type '{artifact_type}': {}",
                    path.display()
                )
            }
            ManifestError::SchemaFileInvalidJson {
                artifact_type,
                path,
                detail,
            } => {
                write!(
                    f,
                    "invalid JSON in schema file for artifact type '{artifact_type}': {}: {detail}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for ManifestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ManifestError::Io(e) => Some(e),
            ManifestError::Parse(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ManifestError {
    fn from(e: std::io::Error) -> Self {
        ManifestError::Io(e)
    }
}

impl From<toml::de::Error> for ManifestError {
    fn from(e: toml::de::Error) -> Self {
        ManifestError::Parse(e)
    }
}

/// Parse a manifest from a file path.
///
/// Reads the file, parses TOML into a `Manifest`, validates that
/// artifact type names and protocol names are unique, and resolves any
/// string-valued schema fields as file paths relative to the manifest
/// directory.
pub fn parse(path: &Path) -> Result<Manifest, ManifestError> {
    let content = std::fs::read_to_string(path)?;
    let mut manifest = from_str(&content)?;
    let manifest_dir = path.parent().unwrap_or(Path::new("."));
    resolve_schema_paths(&mut manifest, manifest_dir)?;
    Ok(manifest)
}

fn resolve_schema_paths(manifest: &mut Manifest, manifest_dir: &Path) -> Result<(), ManifestError> {
    for artifact_type in &mut manifest.artifact_types {
        if let serde_json::Value::String(ref schema_path) = artifact_type.schema {
            let full_path = manifest_dir.join(schema_path);
            let content = std::fs::read_to_string(&full_path).map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    ManifestError::SchemaFileNotFound {
                        artifact_type: artifact_type.name.clone(),
                        path: full_path.clone(),
                    }
                } else {
                    ManifestError::Io(e)
                }
            })?;
            let schema: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
                ManifestError::SchemaFileInvalidJson {
                    artifact_type: artifact_type.name.clone(),
                    path: full_path,
                    detail: e.to_string(),
                }
            })?;
            artifact_type.schema = schema;
        }
    }
    Ok(())
}

/// Parse a manifest from a TOML string.
///
/// Parses the string into a `Manifest` and validates that artifact type
/// names and protocol names are unique.
pub fn from_str(content: &str) -> Result<Manifest, ManifestError> {
    let manifest: Manifest = toml::from_str(content)?;
    validate(&manifest)?;
    Ok(manifest)
}

fn validate(manifest: &Manifest) -> Result<(), ManifestError> {
    let mut seen = HashSet::new();
    for at in &manifest.artifact_types {
        if !seen.insert(&at.name) {
            return Err(ManifestError::DuplicateArtifactType(at.name.clone()));
        }
    }

    let mut seen = HashSet::new();
    for protocol in &manifest.protocols {
        if !seen.insert(&protocol.name) {
            return Err(ManifestError::DuplicateProtocolName(protocol.name.clone()));
        }
        validate_trigger(&protocol.trigger)?;
    }

    Ok(())
}

fn validate_trigger(trigger: &TriggerCondition) -> Result<(), ManifestError> {
    match trigger {
        TriggerCondition::OnSignal { name } => {
            if is_valid_signal_name(name) {
                Ok(())
            } else {
                Err(ManifestError::InvalidSignalName(name.clone()))
            }
        }
        TriggerCondition::AllOf { conditions } | TriggerCondition::AnyOf { conditions } => {
            for condition in conditions {
                validate_trigger(condition)?;
            }
            Ok(())
        }
        TriggerCondition::OnArtifact { .. }
        | TriggerCondition::OnChange { .. }
        | TriggerCondition::OnInvalid { .. } => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ArtifactType, ProtocolDeclaration, TriggerCondition};

    #[test]
    fn parse_valid_manifest() {
        let toml = r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"

[artifact_types.schema]
type = "object"
required = ["description"]

[artifact_types.schema.properties.description]
type = "string"

[[artifact_types]]
name = "design-doc"

[artifact_types.schema]
type = "object"
required = ["content"]

[artifact_types.schema.properties.content]
type = "string"

[[protocols]]
name = "ground"
produces = ["constraints"]

[protocols.trigger]
type = "on_signal"
name = "init"

[[protocols]]
name = "design"
requires = ["constraints"]
accepts = ["prior-design"]
produces = ["design-doc"]
may_produce = ["design-notes"]

[protocols.trigger]
type = "on_artifact"
name = "constraints"
"#;
        let manifest = from_str(toml).unwrap();
        assert_eq!(manifest.name, "groundwork");
        assert_eq!(manifest.artifact_types.len(), 2);
        assert_eq!(manifest.artifact_types[0].name, "constraints");
        assert_eq!(manifest.artifact_types[1].name, "design-doc");
        assert_eq!(manifest.protocols.len(), 2);
        assert_eq!(manifest.protocols[0].name, "ground");
        assert_eq!(manifest.protocols[0].produces, vec!["constraints"]);
        assert_eq!(
            manifest.protocols[0].trigger,
            TriggerCondition::OnSignal {
                name: "init".into()
            }
        );
        assert_eq!(manifest.protocols[1].name, "design");
        assert_eq!(manifest.protocols[1].requires, vec!["constraints"]);
        assert_eq!(manifest.protocols[1].accepts, vec!["prior-design"]);
        assert_eq!(manifest.protocols[1].produces, vec!["design-doc"]);
        assert_eq!(manifest.protocols[1].may_produce, vec!["design-notes"]);
        assert_eq!(
            manifest.protocols[1].trigger,
            TriggerCondition::OnArtifact {
                name: "constraints".into()
            }
        );
    }

    #[test]
    fn parse_manifest_from_file() {
        let toml = r#"
name = "test-methodology"

[[artifact_types]]
name = "report"
schema = { type = "object" }

[[protocols]]
name = "generate"
produces = ["report"]
trigger = { type = "on_signal", name = "go" }
"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.toml");
        std::fs::write(&path, toml).unwrap();

        let manifest = parse(&path).unwrap();
        assert_eq!(manifest.name, "test-methodology");
        assert_eq!(manifest.artifact_types.len(), 1);
        assert_eq!(manifest.protocols.len(), 1);
    }

    #[test]
    fn parse_missing_required_field() {
        // Missing `name` field at top level.
        let toml = r#"
[[artifact_types]]
name = "x"
schema = { type = "object" }

[[protocols]]
name = "y"
trigger = { type = "on_signal", name = "go" }
"#;
        let err = from_str(&toml).unwrap_err();
        assert!(
            matches!(err, ManifestError::Parse(_)),
            "expected Parse error, got: {err}"
        );
    }

    #[test]
    fn parse_legacy_skills_key_is_rejected() {
        let toml = format!(
            r#"
name = "legacy"

[[artifact_types]]
name = "thing"
schema = {{ type = "object" }}

{legacy_key}
name = "do-it"
produces = ["thing"]
trigger = {{ type = "on_signal", name = "go" }}
"#,
            legacy_key = concat!("[[", "skills", "]]"),
        );
        let err = from_str(&toml).unwrap_err();
        assert!(
            matches!(err, ManifestError::Parse(_)),
            "expected Parse error, got: {err}"
        );
        assert!(
            err.to_string().contains("protocols"),
            "expected legacy-key error to mention protocols, got: {err}"
        );
    }

    #[test]
    fn parse_invalid_trigger_type() {
        let toml = r#"
name = "bad"

[[artifact_types]]
name = "x"
schema = { type = "object" }

[[protocols]]
name = "y"
trigger = { type = "bogus", name = "whatever" }
"#;
        let err = from_str(toml).unwrap_err();
        assert!(
            matches!(err, ManifestError::Parse(_)),
            "expected Parse error, got: {err}"
        );
    }

    #[test]
    fn parse_invalid_signal_name_rejects_manifest() {
        let toml = r#"
name = "bad"

[[artifact_types]]
name = "x"
schema = { type = "object" }

[[protocols]]
name = "y"
trigger = { type = "on_signal", name = "release/v1" }
"#;
        let err = from_str(toml).unwrap_err();
        assert!(
            matches!(err, ManifestError::InvalidSignalName(ref name) if name == "release/v1"),
            "expected InvalidSignalName, got: {err}"
        );
    }

    #[test]
    fn parse_duplicate_artifact_type_names() {
        let toml = r#"
name = "dupes"

[[artifact_types]]
name = "report"
schema = { type = "object" }

[[artifact_types]]
name = "report"
schema = { type = "string" }

[[protocols]]
name = "gen"
trigger = { type = "on_signal", name = "go" }
"#;
        let err = from_str(toml).unwrap_err();
        match err {
            ManifestError::DuplicateArtifactType(name) => assert_eq!(name, "report"),
            other => panic!("expected DuplicateArtifactType, got: {other}"),
        }
    }

    #[test]
    fn parse_duplicate_protocol_names() {
        let toml = r#"
name = "dupes"

[[artifact_types]]
name = "x"
schema = { type = "object" }

[[protocols]]
name = "do-thing"
trigger = { type = "on_signal", name = "go" }

[[protocols]]
name = "do-thing"
trigger = { type = "on_signal", name = "start" }
"#;
        let err = from_str(toml).unwrap_err();
        match err {
            ManifestError::DuplicateProtocolName(name) => assert_eq!(name, "do-thing"),
            other => panic!("expected DuplicateProtocolName, got: {other}"),
        }
    }

    #[test]
    fn parse_nested_trigger_conditions() {
        let toml = r#"
name = "nested"

[[artifact_types]]
name = "constraints"
schema = { type = "object" }

[[artifact_types]]
name = "auto-approve"
schema = { type = "object" }

[[protocols]]
name = "deploy"
requires = ["constraints"]

[protocols.trigger]
type = "all_of"
conditions = [
    { type = "on_artifact", name = "constraints" },
    { type = "any_of", conditions = [
        { type = "on_signal", name = "approved" },
        { type = "on_artifact", name = "auto-approve" },
    ]},
]
"#;
        let manifest = from_str(toml).unwrap();
        let trigger = &manifest.protocols[0].trigger;
        assert_eq!(
            *trigger,
            TriggerCondition::AllOf {
                conditions: vec![
                    TriggerCondition::OnArtifact {
                        name: "constraints".into(),
                    },
                    TriggerCondition::AnyOf {
                        conditions: vec![
                            TriggerCondition::OnSignal {
                                name: "approved".into(),
                            },
                            TriggerCondition::OnArtifact {
                                name: "auto-approve".into(),
                            },
                        ],
                    },
                ],
            }
        );
    }

    #[test]
    fn round_trip() {
        let manifest = Manifest {
            name: "round-trip-test".into(),
            artifact_types: vec![
                ArtifactType {
                    name: "constraints".into(),
                    schema: serde_json::json!({
                        "type": "object",
                        "required": ["description"],
                        "properties": {
                            "description": { "type": "string" }
                        }
                    }),
                },
                ArtifactType {
                    name: "report".into(),
                    schema: serde_json::json!({ "type": "object" }),
                },
            ],
            protocols: vec![ProtocolDeclaration {
                name: "analyze".into(),
                requires: vec!["constraints".into()],
                accepts: vec![],
                produces: vec!["report".into()],
                may_produce: vec![],
                trigger: TriggerCondition::OnArtifact {
                    name: "constraints".into(),
                },
            }],
        };

        let toml_string = toml::to_string(&manifest).unwrap();
        let parsed = from_str(&toml_string).unwrap();
        assert_eq!(manifest, parsed);
    }

    #[test]
    fn parse_resolves_file_path_schema() {
        let dir = tempfile::tempdir().unwrap();
        let schemas_dir = dir.path().join("schemas");
        std::fs::create_dir(&schemas_dir).unwrap();
        std::fs::write(
            schemas_dir.join("thing.schema.json"),
            r#"{"type": "object", "required": ["name"], "properties": {"name": {"type": "string"}}}"#,
        )
        .unwrap();

        let toml = r#"
name = "file-schema-test"

[[artifact_types]]
name = "thing"
schema = "schemas/thing.schema.json"

[[protocols]]
name = "make-thing"
produces = ["thing"]
trigger = { type = "on_signal", name = "go" }
"#;
        let manifest_path = dir.path().join("manifest.toml");
        std::fs::write(&manifest_path, toml).unwrap();

        let manifest = parse(&manifest_path).unwrap();
        let schema = &manifest.artifact_types[0].schema;
        assert!(schema.is_object(), "expected object, got: {schema}");
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"][0], "name");
    }

    #[test]
    fn parse_missing_schema_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let toml = r#"
name = "missing-schema"

[[artifact_types]]
name = "thing"
schema = "schemas/nonexistent.schema.json"

[[protocols]]
name = "make-thing"
produces = ["thing"]
trigger = { type = "on_signal", name = "go" }
"#;
        let manifest_path = dir.path().join("manifest.toml");
        std::fs::write(&manifest_path, toml).unwrap();

        let err = parse(&manifest_path).unwrap_err();
        match err {
            ManifestError::SchemaFileNotFound {
                artifact_type,
                path: _,
            } => assert_eq!(artifact_type, "thing"),
            other => panic!("expected SchemaFileNotFound, got: {other}"),
        }
    }

    #[test]
    fn parse_invalid_json_schema_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bad.json"), "{not json").unwrap();

        let toml = r#"
name = "bad-json"

[[artifact_types]]
name = "thing"
schema = "bad.json"

[[protocols]]
name = "make-thing"
produces = ["thing"]
trigger = { type = "on_signal", name = "go" }
"#;
        let manifest_path = dir.path().join("manifest.toml");
        std::fs::write(&manifest_path, toml).unwrap();

        let err = parse(&manifest_path).unwrap_err();
        match err {
            ManifestError::SchemaFileInvalidJson {
                artifact_type,
                path: _,
                detail: _,
            } => assert_eq!(artifact_type, "thing"),
            other => panic!("expected SchemaFileInvalidJson, got: {other}"),
        }
    }

    #[test]
    fn parse_mixed_inline_and_file_schemas() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("ext.schema.json"),
            r#"{"type": "object", "required": ["url"]}"#,
        )
        .unwrap();

        let toml = r#"
name = "mixed"

[[artifact_types]]
name = "inline-thing"
schema = { type = "object", required = ["name"] }

[[artifact_types]]
name = "file-thing"
schema = "ext.schema.json"

[[protocols]]
name = "do-it"
produces = ["inline-thing", "file-thing"]
trigger = { type = "on_signal", name = "go" }
"#;
        let manifest_path = dir.path().join("manifest.toml");
        std::fs::write(&manifest_path, toml).unwrap();

        let manifest = parse(&manifest_path).unwrap();
        assert!(manifest.artifact_types[0].schema.is_object());
        assert_eq!(manifest.artifact_types[0].schema["type"], "object");
        assert!(manifest.artifact_types[1].schema.is_object());
        assert_eq!(manifest.artifact_types[1].schema["required"][0], "url");
    }

    #[test]
    fn parse_optional_vec_fields_omitted() {
        let toml = r#"
name = "minimal"

[[artifact_types]]
name = "thing"
schema = { type = "object" }

[[protocols]]
name = "do-it"
produces = ["thing"]
trigger = { type = "on_signal", name = "go" }
"#;
        let manifest = from_str(toml).unwrap();
        let protocol = &manifest.protocols[0];
        assert!(protocol.requires.is_empty());
        assert!(protocol.accepts.is_empty());
        assert_eq!(protocol.produces, vec!["thing"]);
        assert!(protocol.may_produce.is_empty());
    }
}
