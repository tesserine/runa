use std::collections::HashSet;
use std::fmt;
use std::path::Path;

use crate::model::Manifest;

/// Errors that can occur when parsing a manifest file.
#[derive(Debug)]
pub enum ManifestError {
    /// Failed to read the manifest file.
    Io(std::io::Error),
    /// Failed to parse TOML or map it to the manifest structure.
    Parse(toml::de::Error),
    /// Two artifact types share the same name.
    DuplicateArtifactType(String),
    /// Two skills share the same name.
    DuplicateSkillName(String),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestError::Io(e) => write!(f, "failed to read manifest: {e}"),
            ManifestError::Parse(e) => write!(f, "invalid manifest: {e}"),
            ManifestError::DuplicateArtifactType(name) => {
                write!(f, "duplicate artifact type name: {name}")
            }
            ManifestError::DuplicateSkillName(name) => {
                write!(f, "duplicate skill name: {name}")
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
/// Reads the file, parses TOML into a `Manifest`, and validates that
/// artifact type names and skill names are unique.
pub fn parse(path: &Path) -> Result<Manifest, ManifestError> {
    let content = std::fs::read_to_string(path)?;
    from_str(&content)
}

/// Parse a manifest from a TOML string.
///
/// Parses the string into a `Manifest` and validates that artifact type
/// names and skill names are unique.
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
    for skill in &manifest.skills {
        if !seen.insert(&skill.name) {
            return Err(ManifestError::DuplicateSkillName(skill.name.clone()));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ArtifactType, SkillDeclaration, TriggerCondition};

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

[[skills]]
name = "ground"
produces = ["constraints"]

[skills.trigger]
type = "on_signal"
name = "init"

[[skills]]
name = "design"
requires = ["constraints"]
accepts = ["prior-design"]
produces = ["design-doc"]
may_produce = ["design-notes"]

[skills.trigger]
type = "on_artifact"
name = "constraints"
"#;
        let manifest = from_str(toml).unwrap();
        assert_eq!(manifest.name, "groundwork");
        assert_eq!(manifest.artifact_types.len(), 2);
        assert_eq!(manifest.artifact_types[0].name, "constraints");
        assert_eq!(manifest.artifact_types[1].name, "design-doc");
        assert_eq!(manifest.skills.len(), 2);
        assert_eq!(manifest.skills[0].name, "ground");
        assert_eq!(manifest.skills[0].produces, vec!["constraints"]);
        assert_eq!(
            manifest.skills[0].trigger,
            TriggerCondition::OnSignal {
                name: "init".into()
            }
        );
        assert_eq!(manifest.skills[1].name, "design");
        assert_eq!(manifest.skills[1].requires, vec!["constraints"]);
        assert_eq!(manifest.skills[1].accepts, vec!["prior-design"]);
        assert_eq!(manifest.skills[1].produces, vec!["design-doc"]);
        assert_eq!(manifest.skills[1].may_produce, vec!["design-notes"]);
        assert_eq!(
            manifest.skills[1].trigger,
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

[[skills]]
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
        assert_eq!(manifest.skills.len(), 1);
    }

    #[test]
    fn parse_missing_required_field() {
        // Missing `name` field at top level.
        let toml = r#"
[[artifact_types]]
name = "x"
schema = { type = "object" }

[[skills]]
name = "y"
trigger = { type = "on_signal", name = "go" }
"#;
        let err = from_str(toml).unwrap_err();
        assert!(
            matches!(err, ManifestError::Parse(_)),
            "expected Parse error, got: {err}"
        );
    }

    #[test]
    fn parse_invalid_trigger_type() {
        let toml = r#"
name = "bad"

[[artifact_types]]
name = "x"
schema = { type = "object" }

[[skills]]
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
schema = { type = "object" }

[[artifact_types]]
name = "report"
schema = { type = "string" }

[[skills]]
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
    fn parse_duplicate_skill_names() {
        let toml = r#"
name = "dupes"

[[artifact_types]]
name = "x"
schema = { type = "object" }

[[skills]]
name = "do-thing"
trigger = { type = "on_signal", name = "go" }

[[skills]]
name = "do-thing"
trigger = { type = "on_signal", name = "start" }
"#;
        let err = from_str(toml).unwrap_err();
        match err {
            ManifestError::DuplicateSkillName(name) => assert_eq!(name, "do-thing"),
            other => panic!("expected DuplicateSkillName, got: {other}"),
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

[[skills]]
name = "deploy"
requires = ["constraints"]

[skills.trigger]
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
        let trigger = &manifest.skills[0].trigger;
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
            skills: vec![SkillDeclaration {
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
    fn parse_optional_vec_fields_omitted() {
        let toml = r#"
name = "minimal"

[[artifact_types]]
name = "thing"
schema = { type = "object" }

[[skills]]
name = "do-it"
produces = ["thing"]
trigger = { type = "on_signal", name = "go" }
"#;
        let manifest = from_str(toml).unwrap();
        let skill = &manifest.skills[0];
        assert!(skill.requires.is_empty());
        assert!(skill.accepts.is_empty());
        assert_eq!(skill.produces, vec!["thing"]);
        assert!(skill.may_produce.is_empty());
    }
}
