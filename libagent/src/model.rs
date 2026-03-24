use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A methodology's complete registration with the runa runtime.
///
/// The manifest is the methodology's only interface with the runtime.
/// It declares the methodology's artifact types and protocol declarations.
/// runa reads it, builds the dependency graph, and begins monitoring.
///
/// Format: TOML. See `manifest::parse` for reading from files.
/// TOML serialization of this runtime model is not a supported operation:
/// `manifest::parse` populates schema content from the methodology layout
/// convention, and that derived state is intentionally not accepted back
/// through the TOML manifest parser.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// Methodology name.
    pub name: String,
    /// Artifact types declared by this methodology.
    pub artifact_types: Vec<ArtifactType>,
    /// Protocols declared by this methodology.
    pub protocols: Vec<ProtocolDeclaration>,
}

/// A named category of work product with a machine-checkable schema contract.
///
/// Methodologies define artifact types. The runtime validates instances
/// against their schemas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactType {
    /// Unique identifier within the methodology.
    pub name: String,
    /// JSON Schema defining what a valid instance contains.
    pub schema: serde_json::Value,
}

/// A protocol's declared relationship to artifacts and its activation condition.
///
/// Protocols declare what they require, accept, produce, and may produce.
/// Topology emerges from the graph of these relationships across protocols.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProtocolDeclaration {
    /// Unique identifier for the protocol.
    pub name: String,
    /// Artifact types that must exist and validate before execution.
    #[serde(default)]
    pub requires: Vec<String>,
    /// Artifact types consumed if available; protocol operates without them.
    #[serde(default)]
    pub accepts: Vec<String>,
    /// Artifact types that must exist and validate after execution.
    #[serde(default)]
    pub produces: Vec<String>,
    /// Artifact types that may be produced; validated if present.
    #[serde(default)]
    pub may_produce: Vec<String>,
    /// Condition that activates this protocol.
    pub trigger: TriggerCondition,
    /// Filesystem path to the protocol's instruction content.
    /// Derived from the methodology layout convention by `manifest::parse`.
    /// `None` when produced by `manifest::from_str` (no filesystem access).
    #[serde(skip)]
    pub instructions: Option<PathBuf>,
}

/// Defines when the runtime should activate a protocol.
///
/// Primitive conditions test artifact state or external events.
/// Composite conditions combine primitives with `AllOf` and `AnyOf`.
/// Nesting is permitted to arbitrary depth.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TriggerCondition {
    /// The named artifact exists and satisfies its schema.
    OnArtifact { name: String },
    /// The named artifact is newer than the protocol's current output artifacts.
    OnChange { name: String },
    /// The named artifact exists but fails schema validation.
    OnInvalid { name: String },
    /// All conditions must be satisfied.
    AllOf { conditions: Vec<TriggerCondition> },
    /// At least one condition must be satisfied.
    AnyOf { conditions: Vec<TriggerCondition> },
}

impl fmt::Display for TriggerCondition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TriggerCondition::OnArtifact { name } => write!(f, "on_artifact({name})"),
            TriggerCondition::OnChange { name } => write!(f, "on_change({name})"),
            TriggerCondition::OnInvalid { name } => write!(f, "on_invalid({name})"),
            TriggerCondition::AllOf { conditions } => {
                write!(f, "all_of(")?;
                for (i, c) in conditions.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{c}")?;
                }
                write!(f, ")")
            }
            TriggerCondition::AnyOf { conditions } => {
                write!(f, "any_of(")?;
                for (i, c) in conditions.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{c}")?;
                }
                write!(f, ")")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_type_round_trip() {
        let at = ArtifactType {
            name: "constraints".into(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "description": { "type": "string" }
                },
                "required": ["description"]
            }),
        };
        let json = serde_json::to_string(&at).unwrap();
        let deserialized: ArtifactType = serde_json::from_str(&json).unwrap();
        assert_eq!(at, deserialized);
    }

    #[test]
    fn protocol_declaration_round_trip() {
        let protocol = ProtocolDeclaration {
            name: "design".into(),
            requires: vec!["constraints".into()],
            accepts: vec!["prior-design".into()],
            produces: vec!["design-doc".into()],
            may_produce: vec!["design-notes".into()],
            trigger: TriggerCondition::OnArtifact {
                name: "constraints".into(),
            },
            instructions: None,
        };
        let json = serde_json::to_string(&protocol).unwrap();
        let deserialized: ProtocolDeclaration = serde_json::from_str(&json).unwrap();
        assert_eq!(protocol, deserialized);
    }

    #[test]
    fn trigger_condition_simple_round_trip() {
        let cases = vec![
            TriggerCondition::OnArtifact {
                name: "constraints".into(),
            },
            TriggerCondition::OnChange {
                name: "design-doc".into(),
            },
            TriggerCondition::OnInvalid {
                name: "test-evidence".into(),
            },
        ];
        for tc in cases {
            let json = serde_json::to_string(&tc).unwrap();
            let deserialized: TriggerCondition = serde_json::from_str(&json).unwrap();
            assert_eq!(tc, deserialized);
        }
    }

    #[test]
    fn trigger_condition_nested_round_trip() {
        let tc = TriggerCondition::AllOf {
            conditions: vec![
                TriggerCondition::OnArtifact {
                    name: "constraints".into(),
                },
                TriggerCondition::AnyOf {
                    conditions: vec![
                        TriggerCondition::OnInvalid {
                            name: "draft".into(),
                        },
                        TriggerCondition::OnArtifact {
                            name: "auto-approve".into(),
                        },
                    ],
                },
            ],
        };
        let json = serde_json::to_string(&tc).unwrap();
        let deserialized: TriggerCondition = serde_json::from_str(&json).unwrap();
        assert_eq!(tc, deserialized);
    }

    #[test]
    fn trigger_condition_json_shape() {
        let tc = TriggerCondition::OnArtifact {
            name: "constraints".into(),
        };
        let value = serde_json::to_value(&tc).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "type": "on_artifact",
                "name": "constraints"
            })
        );

        let tc = TriggerCondition::AllOf {
            conditions: vec![TriggerCondition::OnInvalid {
                name: "report".into(),
            }],
        };
        let value = serde_json::to_value(&tc).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "type": "all_of",
                "conditions": [
                    { "type": "on_invalid", "name": "report" }
                ]
            })
        );
    }

    #[test]
    fn protocol_declaration_empty_vecs() {
        let protocol = ProtocolDeclaration {
            name: "artifact-only".into(),
            requires: vec![],
            accepts: vec![],
            produces: vec![],
            may_produce: vec![],
            trigger: TriggerCondition::OnArtifact {
                name: "constraints".into(),
            },
            instructions: None,
        };
        let json = serde_json::to_string(&protocol).unwrap();
        let deserialized: ProtocolDeclaration = serde_json::from_str(&json).unwrap();
        assert_eq!(protocol, deserialized);
    }

    #[test]
    fn display_on_artifact() {
        let tc = TriggerCondition::OnArtifact {
            name: "constraints".into(),
        };
        assert_eq!(tc.to_string(), "on_artifact(constraints)");
    }

    #[test]
    fn display_on_change() {
        let tc = TriggerCondition::OnChange {
            name: "design-doc".into(),
        };
        assert_eq!(tc.to_string(), "on_change(design-doc)");
    }

    #[test]
    fn display_on_invalid() {
        let tc = TriggerCondition::OnInvalid {
            name: "test-evidence".into(),
        };
        assert_eq!(tc.to_string(), "on_invalid(test-evidence)");
    }

    #[test]
    fn display_nested_composite() {
        let tc = TriggerCondition::AllOf {
            conditions: vec![
                TriggerCondition::OnArtifact { name: "X".into() },
                TriggerCondition::AnyOf {
                    conditions: vec![
                        TriggerCondition::OnInvalid { name: "Z".into() },
                        TriggerCondition::OnArtifact { name: "Y".into() },
                    ],
                },
            ],
        };
        assert_eq!(
            tc.to_string(),
            "all_of(on_artifact(X), any_of(on_invalid(Z), on_artifact(Y)))"
        );
    }
}
