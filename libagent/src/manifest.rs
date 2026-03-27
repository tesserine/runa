//! TOML manifest parsing, structural validation, and methodology layout resolution.
//!
//! Converts a methodology manifest file into [`Manifest`] model types,
//! enforcing name uniqueness and path-safety constraints, then resolving schema files
//! and protocol instruction files from the methodology's directory layout.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::model::{ArtifactType, Manifest, ProtocolDeclaration, TriggerCondition};

// ---------------------------------------------------------------------------
// Raw TOML deserialization types
//
// These mirror the model types but represent only what appears in the TOML
// manifest. Schema content and protocol instructions are derived from the
// methodology layout convention during `parse()`, not declared in TOML.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RawManifest {
    name: String,
    artifact_types: Vec<RawArtifactType>,
    protocols: Vec<RawProtocolDeclaration>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawArtifactType {
    name: String,
}

#[derive(Deserialize)]
struct RawProtocolDeclaration {
    name: String,
    #[serde(default)]
    requires: Vec<String>,
    #[serde(default)]
    accepts: Vec<String>,
    #[serde(default)]
    produces: Vec<String>,
    #[serde(default)]
    may_produce: Vec<String>,
    trigger: TriggerCondition,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors that can occur when parsing a manifest file.
#[derive(Debug)]
pub enum ManifestError {
    /// Failed to read the manifest file.
    Io(std::io::Error),
    /// Failed to parse TOML or map it to the manifest structure.
    Parse(toml::de::Error),
    /// Artifact type name is unsafe as a layout-derived path component.
    InvalidArtifactTypeName(String),
    /// Two artifact types share the same name.
    DuplicateArtifactType(String),
    /// Protocol name is unsafe as a layout-derived path component.
    InvalidProtocolName(String),
    /// Two protocols share the same name.
    DuplicateProtocolName(String),
    /// Schema file missing at its conventional location.
    SchemaNotFound {
        artifact_type: String,
        expected_path: PathBuf,
    },
    /// Schema file exists but contains invalid JSON.
    SchemaInvalidJson {
        artifact_type: String,
        path: PathBuf,
        detail: String,
    },
    /// Instruction file missing at its conventional location.
    InstructionFileNotFound {
        protocol: String,
        expected_path: PathBuf,
    },
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestError::Io(e) => write!(f, "failed to read manifest: {e}"),
            ManifestError::Parse(e) => write!(f, "invalid manifest: {e}"),
            ManifestError::InvalidArtifactTypeName(name) => write!(
                f,
                "invalid artifact type name '{name}': names must not contain '/', '\\', or '..'"
            ),
            ManifestError::DuplicateArtifactType(name) => {
                write!(f, "duplicate artifact type name: {name}")
            }
            ManifestError::InvalidProtocolName(name) => write!(
                f,
                "invalid protocol name '{name}': names must not contain '/', '\\', or '..'"
            ),
            ManifestError::DuplicateProtocolName(name) => {
                write!(f, "duplicate protocol name: {name}")
            }
            ManifestError::SchemaNotFound {
                artifact_type,
                expected_path,
            } => {
                write!(
                    f,
                    "schema file not found for artifact type '{artifact_type}' \
                     at expected location: {}",
                    expected_path.display()
                )
            }
            ManifestError::SchemaInvalidJson {
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
            ManifestError::InstructionFileNotFound {
                protocol,
                expected_path,
            } => {
                write!(
                    f,
                    "instruction file not found for protocol '{protocol}' \
                     at expected location: {}",
                    expected_path.display()
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

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Parse a manifest from a file path.
///
/// Reads the file, parses TOML into a `Manifest`, validates declared artifact
/// type and protocol names are unique and safe as layout-derived path
/// components, then resolves the methodology layout convention: loads schema
/// content from `schemas/{artifact_type_name}.schema.json` and validates
/// instruction file existence at `protocols/{protocol_name}/PROTOCOL.md`, both
/// relative to the manifest directory.
pub fn parse(path: &Path) -> Result<Manifest, ManifestError> {
    let content = std::fs::read_to_string(path)?;
    let mut manifest = from_str(&content)?;
    let manifest_dir = path.parent().unwrap_or(Path::new("."));
    resolve_methodology_layout(&mut manifest, manifest_dir)?;
    Ok(manifest)
}

/// Resolve schema content and protocol instructions from the methodology layout
/// convention.
fn resolve_methodology_layout(
    manifest: &mut Manifest,
    manifest_dir: &Path,
) -> Result<(), ManifestError> {
    // Schemas: schemas/{artifact_type_name}.schema.json
    for artifact_type in &mut manifest.artifact_types {
        let schema_path = manifest_dir
            .join("schemas")
            .join(format!("{}.schema.json", artifact_type.name));
        let content = std::fs::read_to_string(&schema_path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ManifestError::SchemaNotFound {
                    artifact_type: artifact_type.name.clone(),
                    expected_path: schema_path.clone(),
                }
            } else {
                ManifestError::Io(e)
            }
        })?;
        let schema: serde_json::Value =
            serde_json::from_str(&content).map_err(|e| ManifestError::SchemaInvalidJson {
                artifact_type: artifact_type.name.clone(),
                path: schema_path,
                detail: e.to_string(),
            })?;
        artifact_type.schema = schema;
    }

    // Instructions: protocols/{protocol_name}/PROTOCOL.md
    for protocol in &mut manifest.protocols {
        let instruction_path = manifest_dir
            .join("protocols")
            .join(&protocol.name)
            .join("PROTOCOL.md");
        let content = std::fs::read_to_string(&instruction_path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ManifestError::InstructionFileNotFound {
                    protocol: protocol.name.clone(),
                    expected_path: instruction_path.clone(),
                }
            } else {
                ManifestError::Io(e)
            }
        })?;
        protocol.instructions = Some(content);
    }

    Ok(())
}

/// Parse a manifest from a TOML string.
///
/// Deserializes the TOML into a `Manifest` and validates that artifact type
/// names and protocol names are unique and safe as layout-derived path
/// components. Schema content and protocol instructions are not resolved — use
/// `parse` for a complete manifest with filesystem resolution.
///
/// The returned `Manifest` is an intermediate runtime shape for the parsing
/// pipeline, not a fully resolved methodology registration: every artifact
/// type schema is left as `serde_json::Value::Null`, every protocol
/// instruction field is `None`, and the result is therefore not suitable for
/// artifact validation or protocol execution without a subsequent
/// filesystem-backed `parse`.
pub fn from_str(content: &str) -> Result<Manifest, ManifestError> {
    let raw: RawManifest = toml::from_str(content)?;
    let manifest = Manifest {
        name: raw.name,
        artifact_types: raw
            .artifact_types
            .into_iter()
            .map(|r| ArtifactType {
                name: r.name,
                schema: serde_json::Value::Null,
            })
            .collect(),
        protocols: raw
            .protocols
            .into_iter()
            .map(|r| ProtocolDeclaration {
                name: r.name,
                requires: r.requires,
                accepts: r.accepts,
                produces: r.produces,
                may_produce: r.may_produce,
                trigger: r.trigger,
                instructions: None,
            })
            .collect(),
    };
    validate(&manifest)?;
    Ok(manifest)
}

fn validate(manifest: &Manifest) -> Result<(), ManifestError> {
    let mut seen = HashSet::new();
    for at in &manifest.artifact_types {
        validate_layout_name(&at.name, NameKind::ArtifactType)?;
        if !seen.insert(&at.name) {
            return Err(ManifestError::DuplicateArtifactType(at.name.clone()));
        }
    }

    let mut seen = HashSet::new();
    for protocol in &manifest.protocols {
        validate_layout_name(&protocol.name, NameKind::Protocol)?;
        if !seen.insert(&protocol.name) {
            return Err(ManifestError::DuplicateProtocolName(protocol.name.clone()));
        }
    }

    Ok(())
}

#[derive(Copy, Clone)]
enum NameKind {
    ArtifactType,
    Protocol,
}

fn validate_layout_name(name: &str, kind: NameKind) -> Result<(), ManifestError> {
    if name.is_empty()
        || name == "."
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
    {
        return Err(match kind {
            NameKind::ArtifactType => ManifestError::InvalidArtifactTypeName(name.to_string()),
            NameKind::Protocol => ManifestError::InvalidProtocolName(name.to_string()),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TriggerCondition;

    /// Create a methodology layout in `dir` with schema and protocol instruction files.
    fn write_layout(dir: &Path, schemas: &[(&str, &str)], protocols: &[&str]) {
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
    }

    #[test]
    fn from_str_parses_valid_manifest() {
        let toml = r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"

[[artifact_types]]
name = "design-doc"

[[protocols]]
name = "ground"
produces = ["constraints"]

[protocols.trigger]
type = "on_change"
name = "constraints"

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
        // from_str does not resolve schemas — they remain null.
        assert_eq!(manifest.artifact_types[0].schema, serde_json::Value::Null);
        assert_eq!(manifest.artifact_types[1].schema, serde_json::Value::Null);
        assert_eq!(manifest.protocols.len(), 2);
        assert_eq!(manifest.protocols[0].name, "ground");
        assert_eq!(manifest.protocols[0].produces, vec!["constraints"]);
        assert_eq!(
            manifest.protocols[0].trigger,
            TriggerCondition::OnChange {
                name: "constraints".into()
            }
        );
        assert_eq!(manifest.protocols[0].instructions, None);
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
    fn parse_resolves_convention_layout() {
        let dir = tempfile::tempdir().unwrap();
        write_layout(
            dir.path(),
            &[("report", r#"{"type": "object", "required": ["title"]}"#)],
            &["generate"],
        );

        let toml = r#"
name = "test-methodology"

[[artifact_types]]
name = "report"

[[protocols]]
name = "generate"
produces = ["report"]
trigger = { type = "on_change", name = "report" }
"#;
        let manifest_path = dir.path().join("manifest.toml");
        std::fs::write(&manifest_path, toml).unwrap();

        let manifest = parse(&manifest_path).unwrap();
        assert_eq!(manifest.name, "test-methodology");

        // Schema loaded from convention path.
        let schema = &manifest.artifact_types[0].schema;
        assert!(schema.is_object(), "expected object, got: {schema}");
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"][0], "title");

        // Instruction content loaded from convention path.
        let instructions = manifest.protocols[0].instructions.as_ref().unwrap();
        assert_eq!(instructions, "# generate\n");
    }

    #[test]
    fn parse_missing_required_field() {
        // Missing `name` field at top level.
        let toml = r#"
[[artifact_types]]
name = "x"

[[protocols]]
name = "y"
trigger = { type = "on_change", name = "report" }
"#;
        let err = from_str(toml).unwrap_err();
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

{legacy_key}
name = "do-it"
produces = ["thing"]
trigger = {{ type = "on_change", name = "thing" }}
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
    fn parse_rejects_old_format_schema_field() {
        let toml = r#"
name = "old-format"

[[artifact_types]]
name = "thing"
schema = { type = "object" }

[[protocols]]
name = "do-it"
trigger = { type = "on_change", name = "thing" }
"#;
        let err = from_str(toml).unwrap_err();
        assert!(
            matches!(err, ManifestError::Parse(_)),
            "expected Parse error for old-format schema field, got: {err}"
        );
    }

    #[test]
    fn parse_invalid_trigger_type() {
        let toml = r#"
name = "bad"

[[artifact_types]]
name = "x"

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
    fn parse_duplicate_artifact_type_names() {
        let toml = r#"
name = "dupes"

[[artifact_types]]
name = "report"

[[artifact_types]]
name = "report"

[[protocols]]
name = "gen"
trigger = { type = "on_change", name = "report" }
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

[[protocols]]
name = "do-thing"
trigger = { type = "on_change", name = "report" }

[[protocols]]
name = "do-thing"
trigger = { type = "on_change", name = "x" }
"#;
        let err = from_str(toml).unwrap_err();
        match err {
            ManifestError::DuplicateProtocolName(name) => assert_eq!(name, "do-thing"),
            other => panic!("expected DuplicateProtocolName, got: {other}"),
        }
    }

    #[test]
    fn parse_rejects_artifact_type_names_with_unsafe_path_components() {
        for invalid_name in ["foo/bar", r"foo\bar", "foo..bar"] {
            let toml = format!(
                r#"
name = "unsafe-artifact"

[[artifact_types]]
name = '{invalid_name}'

[[protocols]]
name = "generate"
produces = ["safe-output"]
trigger = {{ type = "on_change", name = "safe-output" }}
"#
            );

            let err = from_str(&toml).unwrap_err();
            assert!(
                err.to_string().contains("artifact type name"),
                "expected artifact type name validation error for {invalid_name:?}, got: {err}"
            );
            assert!(
                err.to_string().contains(invalid_name),
                "expected invalid artifact type name in error for {invalid_name:?}, got: {err}"
            );
        }
    }

    #[test]
    fn parse_rejects_protocol_names_with_unsafe_path_components() {
        for invalid_name in ["", ".", "foo/bar", r"foo\bar", "foo..bar"] {
            let toml = format!(
                r#"
name = "unsafe-protocol"

[[artifact_types]]
name = "report"

[[protocols]]
name = '{invalid_name}'
produces = ["report"]
trigger = {{ type = "on_change", name = "report" }}
"#
            );

            let err = from_str(&toml).unwrap_err();
            assert!(
                err.to_string().contains("protocol name"),
                "expected protocol name validation error for {invalid_name:?}, got: {err}"
            );
            assert!(
                err.to_string().contains(invalid_name),
                "expected invalid protocol name in error for {invalid_name:?}, got: {err}"
            );
        }
    }

    #[test]
    fn parse_nested_trigger_conditions() {
        let toml = r#"
name = "nested"

[[artifact_types]]
name = "constraints"

[[artifact_types]]
name = "auto-approve"

[[protocols]]
name = "deploy"
requires = ["constraints"]

[protocols.trigger]
type = "all_of"
conditions = [
    { type = "on_artifact", name = "constraints" },
    { type = "any_of", conditions = [
        { type = "on_invalid", name = "constraints" },
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
                            TriggerCondition::OnInvalid {
                                name: "constraints".into(),
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
    fn parse_resolves_convention_schema() {
        let dir = tempfile::tempdir().unwrap();
        write_layout(
            dir.path(),
            &[(
                "thing",
                r#"{"type": "object", "required": ["name"], "properties": {"name": {"type": "string"}}}"#,
            )],
            &["make-thing"],
        );

        let toml = r#"
name = "convention-schema-test"

[[artifact_types]]
name = "thing"

[[protocols]]
name = "make-thing"
produces = ["thing"]
trigger = { type = "on_change", name = "report" }
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
    fn parse_missing_schema_at_convention_path() {
        let dir = tempfile::tempdir().unwrap();
        // Create instruction file but no schema file.
        write_layout(dir.path(), &[], &["make-thing"]);

        let toml = r#"
name = "missing-schema"

[[artifact_types]]
name = "thing"

[[protocols]]
name = "make-thing"
produces = ["thing"]
trigger = { type = "on_change", name = "report" }
"#;
        let manifest_path = dir.path().join("manifest.toml");
        std::fs::write(&manifest_path, toml).unwrap();

        let err = parse(&manifest_path).unwrap_err();
        match err {
            ManifestError::SchemaNotFound {
                artifact_type,
                expected_path: _,
            } => assert_eq!(artifact_type, "thing"),
            other => panic!("expected SchemaNotFound, got: {other}"),
        }
    }

    #[test]
    fn parse_invalid_json_in_convention_schema() {
        let dir = tempfile::tempdir().unwrap();
        write_layout(dir.path(), &[], &["make-thing"]);
        // Write invalid JSON to the schema file.
        std::fs::write(dir.path().join("schemas/thing.schema.json"), "{not json").unwrap();

        let toml = r#"
name = "bad-json"

[[artifact_types]]
name = "thing"

[[protocols]]
name = "make-thing"
produces = ["thing"]
trigger = { type = "on_change", name = "report" }
"#;
        let manifest_path = dir.path().join("manifest.toml");
        std::fs::write(&manifest_path, toml).unwrap();

        let err = parse(&manifest_path).unwrap_err();
        match err {
            ManifestError::SchemaInvalidJson {
                artifact_type,
                path: _,
                detail: _,
            } => assert_eq!(artifact_type, "thing"),
            other => panic!("expected SchemaInvalidJson, got: {other}"),
        }
    }

    #[test]
    fn parse_missing_instruction_file() {
        let dir = tempfile::tempdir().unwrap();
        // Create schema but no instruction file.
        write_layout(dir.path(), &[("thing", r#"{"type": "object"}"#)], &[]);

        let toml = r#"
name = "missing-instructions"

[[artifact_types]]
name = "thing"

[[protocols]]
name = "make-thing"
produces = ["thing"]
trigger = { type = "on_change", name = "report" }
"#;
        let manifest_path = dir.path().join("manifest.toml");
        std::fs::write(&manifest_path, toml).unwrap();

        let err = parse(&manifest_path).unwrap_err();
        match err {
            ManifestError::InstructionFileNotFound {
                protocol,
                expected_path: _,
            } => assert_eq!(protocol, "make-thing"),
            other => panic!("expected InstructionFileNotFound, got: {other}"),
        }
    }

    #[test]
    fn parse_rejects_unsafe_artifact_names_before_schema_lookup() {
        let dir = tempfile::tempdir().unwrap();
        write_layout(dir.path(), &[], &["generate"]);

        let toml = r#"
name = "unsafe-layout"

[[artifact_types]]
name = "../escaped"

[[protocols]]
name = "generate"
produces = ["../escaped"]
trigger = { type = "on_change", name = "../escaped" }
"#;
        let manifest_path = dir.path().join("manifest.toml");
        std::fs::write(&manifest_path, toml).unwrap();

        let err = parse(&manifest_path).unwrap_err();
        assert!(
            err.to_string().contains("artifact type name"),
            "expected invalid artifact type name error, got: {err}"
        );
        assert!(
            !matches!(err, ManifestError::SchemaNotFound { .. }),
            "unsafe artifact name should fail before schema lookup"
        );
    }

    #[test]
    fn parse_rejects_unsafe_protocol_names_before_instruction_lookup() {
        let dir = tempfile::tempdir().unwrap();
        write_layout(dir.path(), &[("report", r#"{"type": "object"}"#)], &[]);

        let toml = r#"
name = "unsafe-layout"

[[artifact_types]]
name = "report"

[[protocols]]
name = "../escaped"
produces = ["report"]
trigger = { type = "on_change", name = "report" }
"#;
        let manifest_path = dir.path().join("manifest.toml");
        std::fs::write(&manifest_path, toml).unwrap();

        let err = parse(&manifest_path).unwrap_err();
        assert!(
            err.to_string().contains("protocol name"),
            "expected invalid protocol name error, got: {err}"
        );
        assert!(
            !matches!(err, ManifestError::InstructionFileNotFound { .. }),
            "unsafe protocol name should fail before instruction lookup"
        );
    }

    #[test]
    fn parse_rejects_dot_protocol_name_before_instruction_lookup() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("protocols")).unwrap();
        std::fs::write(dir.path().join("protocols/PROTOCOL.md"), "# unrelated\n").unwrap();
        write_layout(dir.path(), &[("report", r#"{"type": "object"}"#)], &[]);

        let toml = r#"
name = "unsafe-layout"

[[artifact_types]]
name = "report"

[[protocols]]
name = "."
produces = ["report"]
trigger = { type = "on_change", name = "report" }
"#;
        let manifest_path = dir.path().join("manifest.toml");
        std::fs::write(&manifest_path, toml).unwrap();

        let err = parse(&manifest_path).unwrap_err();
        assert!(
            err.to_string().contains("protocol name"),
            "expected invalid protocol name error, got: {err}"
        );
        assert!(
            !matches!(err, ManifestError::InstructionFileNotFound { .. }),
            "dot protocol name should fail before instruction lookup"
        );
    }

    #[test]
    fn parse_optional_vec_fields_omitted() {
        let toml = r#"
name = "minimal"

[[artifact_types]]
name = "thing"

[[protocols]]
name = "do-it"
produces = ["thing"]
trigger = { type = "on_change", name = "report" }
"#;
        let manifest = from_str(toml).unwrap();
        let protocol = &manifest.protocols[0];
        assert!(protocol.requires.is_empty());
        assert!(protocol.accepts.is_empty());
        assert_eq!(protocol.produces, vec!["thing"]);
        assert!(protocol.may_produce.is_empty());
    }

    #[test]
    fn parse_resolves_paths_relative_to_manifest_dir() {
        let dir = tempfile::tempdir().unwrap();
        let methodology_dir = dir.path().join("methodology");
        std::fs::create_dir(&methodology_dir).unwrap();

        write_layout(
            &methodology_dir,
            &[("report", r#"{"type": "object"}"#)],
            &["generate"],
        );

        let toml = r#"
name = "relative-test"

[[artifact_types]]
name = "report"

[[protocols]]
name = "generate"
produces = ["report"]
trigger = { type = "on_change", name = "report" }
"#;
        let manifest_path = methodology_dir.join("manifest.toml");
        std::fs::write(&manifest_path, toml).unwrap();

        let manifest = parse(&manifest_path).unwrap();
        assert_eq!(manifest.artifact_types[0].schema["type"], "object");
        let instructions = manifest.protocols[0].instructions.as_ref().unwrap();
        assert_eq!(instructions, "# generate\n");
    }
}
