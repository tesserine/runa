use std::fmt;

use serde::{Deserialize, Serialize};

/// A methodology's complete registration with the runa runtime.
///
/// The manifest is the methodology's only interface with the runtime.
/// It declares the methodology's artifact types and skill declarations.
/// runa reads it, builds the dependency graph, and begins monitoring.
///
/// Format: TOML. See `manifest::parse` for reading from files.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// Methodology name.
    pub name: String,
    /// Artifact types declared by this methodology.
    pub artifact_types: Vec<ArtifactType>,
    /// Skills declared by this methodology.
    pub skills: Vec<SkillDeclaration>,
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

/// A skill's declared relationship to artifacts and its activation condition.
///
/// Skills declare what they require, accept, produce, and may produce.
/// Topology emerges from the graph of these relationships across skills.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillDeclaration {
    /// Unique identifier for the skill.
    pub name: String,
    /// Artifact types that must exist and validate before execution.
    #[serde(default)]
    pub requires: Vec<String>,
    /// Artifact types consumed if available; skill operates without them.
    #[serde(default)]
    pub accepts: Vec<String>,
    /// Artifact types that must exist and validate after execution.
    #[serde(default)]
    pub produces: Vec<String>,
    /// Artifact types that may be produced; validated if present.
    #[serde(default)]
    pub may_produce: Vec<String>,
    /// Condition that activates this skill.
    pub trigger: TriggerCondition,
}

/// Defines when the runtime should activate a skill.
///
/// Primitive conditions test artifact state or external events.
/// Composite conditions combine primitives with `AllOf` and `AnyOf`.
/// Nesting is permitted to arbitrary depth.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TriggerCondition {
    /// The named artifact exists and satisfies its schema.
    OnArtifact { name: String },
    /// The named artifact has been modified since the skill's last activation.
    OnChange { name: String },
    /// The named artifact exists but fails schema validation.
    OnInvalid { name: String },
    /// An external event (operator action, webhook, scheduler).
    OnSignal { name: String },
    /// All conditions must be satisfied.
    AllOf { conditions: Vec<TriggerCondition> },
    /// At least one condition must be satisfied.
    AnyOf { conditions: Vec<TriggerCondition> },
}

pub fn is_valid_signal_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return false;
    }

    chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_')
}

impl fmt::Display for TriggerCondition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TriggerCondition::OnArtifact { name } => write!(f, "on_artifact({name})"),
            TriggerCondition::OnChange { name } => write!(f, "on_change({name})"),
            TriggerCondition::OnInvalid { name } => write!(f, "on_invalid({name})"),
            TriggerCondition::OnSignal { name } => write!(f, "on_signal({name})"),
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
    fn skill_declaration_round_trip() {
        let skill = SkillDeclaration {
            name: "design".into(),
            requires: vec!["constraints".into()],
            accepts: vec!["prior-design".into()],
            produces: vec!["design-doc".into()],
            may_produce: vec!["design-notes".into()],
            trigger: TriggerCondition::OnArtifact {
                name: "constraints".into(),
            },
        };
        let json = serde_json::to_string(&skill).unwrap();
        let deserialized: SkillDeclaration = serde_json::from_str(&json).unwrap();
        assert_eq!(skill, deserialized);
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
            TriggerCondition::OnSignal {
                name: "approved".into(),
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
                        TriggerCondition::OnSignal {
                            name: "approved".into(),
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
            conditions: vec![TriggerCondition::OnSignal { name: "go".into() }],
        };
        let value = serde_json::to_value(&tc).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "type": "all_of",
                "conditions": [
                    { "type": "on_signal", "name": "go" }
                ]
            })
        );
    }

    #[test]
    fn skill_declaration_empty_vecs() {
        let skill = SkillDeclaration {
            name: "signal-only".into(),
            requires: vec![],
            accepts: vec![],
            produces: vec![],
            may_produce: vec![],
            trigger: TriggerCondition::OnSignal {
                name: "manual".into(),
            },
        };
        let json = serde_json::to_string(&skill).unwrap();
        let deserialized: SkillDeclaration = serde_json::from_str(&json).unwrap();
        assert_eq!(skill, deserialized);
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
    fn display_on_signal() {
        let tc = TriggerCondition::OnSignal {
            name: "approved".into(),
        };
        assert_eq!(tc.to_string(), "on_signal(approved)");
    }

    #[test]
    fn display_nested_composite() {
        let tc = TriggerCondition::AllOf {
            conditions: vec![
                TriggerCondition::OnArtifact { name: "X".into() },
                TriggerCondition::AnyOf {
                    conditions: vec![
                        TriggerCondition::OnSignal { name: "go".into() },
                        TriggerCondition::OnArtifact { name: "Y".into() },
                    ],
                },
            ],
        };
        assert_eq!(
            tc.to_string(),
            "all_of(on_artifact(X), any_of(on_signal(go), on_artifact(Y)))"
        );
    }

    #[test]
    fn signal_name_validation_rejects_invalid_edge_cases() {
        assert!(!is_valid_signal_name(""));
        assert!(!is_valid_signal_name("-start"));
        assert!(!is_valid_signal_name("_start"));
        assert!(!is_valid_signal_name("Bad"));
        assert!(!is_valid_signal_name("release/v1"));
    }

    #[test]
    fn signal_name_validation_accepts_underscores_and_hyphens() {
        assert!(is_valid_signal_name("begin"));
        assert!(is_valid_signal_name("qa_ready"));
        assert!(is_valid_signal_name("release-v1"));
        assert!(is_valid_signal_name("1phase_ready"));
    }
}
