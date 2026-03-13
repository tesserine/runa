//! Pre/post-execution enforcement for skill declarations.
//!
//! Enforcement turns methodology declarations into runtime guarantees.
//! Given a skill declaration and the current artifact store state,
//! enforcement checks whether the skill's contracts are satisfied.
//!
//! - Pre-execution: all `requires` artifacts must exist and be valid
//! - Post-execution: all `produces` artifacts must exist and be valid;
//!   `may_produce` artifacts are validated if present but their absence
//!   is not a failure

use std::fmt;

use crate::model::SkillDeclaration;
use crate::store::{ArtifactStore, ValidationStatus};
use crate::validation::Violation;

/// The declared relationship between a skill and an artifact type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Relationship {
    Requires,
    Produces,
    MayProduce,
}

/// Why a single artifact type failed enforcement.
///
/// Each variant indicates a different corrective action:
/// - `Missing` — record this artifact
/// - `Invalid` — fix these schema violations
/// - `Stale` — revalidate this artifact
#[derive(Debug, Clone, PartialEq)]
pub enum ArtifactFailure {
    /// No instances of this artifact type exist in the store.
    Missing {
        artifact_type: String,
        relationship: Relationship,
    },
    /// At least one instance exists but fails schema validation.
    Invalid {
        artifact_type: String,
        relationship: Relationship,
        violations: Vec<Violation>,
    },
    /// At least one instance exists but is stale (needs revalidation).
    Stale {
        artifact_type: String,
        relationship: Relationship,
        stale_count: usize,
    },
}

/// The phase of enforcement that failed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    Pre,
    Post,
}

/// Enforcement failed: a skill's contracts are not satisfied.
///
/// Contains the skill name, enforcement phase, and all artifact
/// failures found. Collects all failures before returning (does
/// not short-circuit on first failure).
#[derive(Debug, Clone, PartialEq)]
pub struct EnforcementError {
    pub skill_name: String,
    pub phase: Phase,
    pub failures: Vec<ArtifactFailure>,
}

impl fmt::Display for ArtifactFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArtifactFailure::Missing {
                artifact_type,
                relationship,
            } => match relationship {
                Relationship::Requires => {
                    write!(
                        f,
                        "requires artifact type '{artifact_type}' which is missing"
                    )
                }
                Relationship::Produces => {
                    write!(
                        f,
                        "declares it produces artifact type '{artifact_type}' \
                         which is missing after execution"
                    )
                }
                Relationship::MayProduce => {
                    unreachable!(
                        "may_produce artifacts are never reported as Missing — \
                         absent may_produce is skipped, not an enforcement failure"
                    )
                }
            },
            ArtifactFailure::Invalid {
                artifact_type,
                relationship,
                violations,
            } => {
                let prefix = match relationship {
                    Relationship::Requires => format!(
                        "requires artifact type '{artifact_type}' \
                         which exists but fails validation"
                    ),
                    Relationship::Produces => format!(
                        "declares it produces artifact type '{artifact_type}' \
                         which exists but is invalid"
                    ),
                    Relationship::MayProduce => format!(
                        "may_produce artifact type '{artifact_type}' \
                         which exists but is invalid"
                    ),
                };
                write!(f, "{prefix}")?;
                for v in violations {
                    write!(f, "\n      {}: {}", v.schema_path, v.description)?;
                }
                Ok(())
            }
            ArtifactFailure::Stale {
                artifact_type,
                relationship,
                stale_count,
            } => {
                let prefix = match relationship {
                    Relationship::Requires => {
                        format!("requires artifact type '{artifact_type}' which is stale")
                    }
                    Relationship::Produces => format!(
                        "declares it produces artifact type '{artifact_type}' which is stale"
                    ),
                    Relationship::MayProduce => {
                        format!("may_produce artifact type '{artifact_type}' which is stale")
                    }
                };
                write!(
                    f,
                    "{prefix} ({stale_count} instance{} need{} revalidation)",
                    if *stale_count == 1 { "" } else { "s" },
                    if *stale_count == 1 { "s" } else { "" },
                )
            }
        }
    }
}

impl fmt::Display for EnforcementError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let phase_label = match self.phase {
            Phase::Pre => "pre-execution",
            Phase::Post => "post-execution",
        };
        write!(
            f,
            "enforcement failed for skill '{}' ({phase_label}):",
            self.skill_name
        )?;
        for failure in &self.failures {
            write!(f, "\n  - {failure}")?;
        }
        Ok(())
    }
}

impl std::error::Error for EnforcementError {}

/// Check a single artifact type against the store and return a failure if
/// it is not fully valid.
fn check_artifact(
    store: &ArtifactStore,
    artifact_type: &str,
    relationship: Relationship,
) -> Option<ArtifactFailure> {
    if store.is_valid(artifact_type) {
        return None;
    }

    let instances = store.instances_of(artifact_type);
    if instances.is_empty() {
        return Some(ArtifactFailure::Missing {
            artifact_type: artifact_type.to_string(),
            relationship,
        });
    }

    // Has instances but not all valid. Classify: invalid (schema violations)
    // takes precedence over stale.
    let mut violations = Vec::new();
    let mut stale_count = 0;
    for (_, state) in &instances {
        match &state.status {
            ValidationStatus::Invalid(vs) => violations.extend(vs.iter().cloned()),
            ValidationStatus::Malformed(error) => violations.push(Violation {
                artifact_type: artifact_type.to_string(),
                description: format!("malformed JSON: {error}"),
                schema_path: "<parse>".to_string(),
                instance_path: String::new(),
            }),
            ValidationStatus::Stale => stale_count += 1,
            ValidationStatus::Valid => {}
        }
    }

    if !violations.is_empty() {
        Some(ArtifactFailure::Invalid {
            artifact_type: artifact_type.to_string(),
            relationship,
            violations,
        })
    } else {
        Some(ArtifactFailure::Stale {
            artifact_type: artifact_type.to_string(),
            relationship,
            stale_count,
        })
    }
}

/// Check that all `requires` artifacts exist and are valid.
///
/// Returns `Ok(())` if every artifact type listed in the skill's `requires`
/// has at least one instance in the store and **all** instances of each
/// required type are valid. One invalid or stale instance of a required
/// type blocks execution.
///
/// `accepts` artifacts are explicitly NOT checked. Their absence is expected
/// behavior — they represent optional inputs that the skill can consume if
/// available, not preconditions for execution.
pub fn enforce_preconditions(
    skill: &SkillDeclaration,
    store: &ArtifactStore,
) -> Result<(), EnforcementError> {
    let mut failures = Vec::new();

    for artifact_type in &skill.requires {
        if let Some(failure) = check_artifact(store, artifact_type, Relationship::Requires) {
            failures.push(failure);
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(EnforcementError {
            skill_name: skill.name.clone(),
            phase: Phase::Pre,
            failures,
        })
    }
}

/// Check that all `produces` artifacts exist and are valid, and that
/// any `may_produce` artifacts that exist are also valid.
///
/// Returns `Ok(())` if:
/// - Every artifact type in `produces` has at least one instance and **all**
///   instances are valid. One invalid or stale instance means the skill's
///   output contract is not satisfied.
/// - Every artifact type in `may_produce` either has no instances or has
///   all-valid instances.
///
/// `accepts` artifacts are explicitly NOT checked. They are input edges,
/// not output edges, and are irrelevant to post-execution enforcement.
pub fn enforce_postconditions(
    skill: &SkillDeclaration,
    store: &ArtifactStore,
) -> Result<(), EnforcementError> {
    let mut failures = Vec::new();

    for artifact_type in &skill.produces {
        if let Some(failure) = check_artifact(store, artifact_type, Relationship::Produces) {
            failures.push(failure);
        }
    }

    for artifact_type in &skill.may_produce {
        // may_produce: absent is ok — only check if instances exist.
        let instances = store.instances_of(artifact_type);
        if instances.is_empty() {
            continue;
        }
        if let Some(failure) = check_artifact(store, artifact_type, Relationship::MayProduce) {
            failures.push(failure);
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(EnforcementError {
            skill_name: skill.name.clone(),
            phase: Phase::Post,
            failures,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TriggerCondition;
    use crate::test_helpers::make_store;
    use serde_json::json;
    use std::path::Path;
    use tempfile::TempDir;

    fn make_skill(
        name: &str,
        requires: &[&str],
        accepts: &[&str],
        produces: &[&str],
        may_produce: &[&str],
    ) -> SkillDeclaration {
        SkillDeclaration {
            name: name.into(),
            requires: requires.iter().map(|s| s.to_string()).collect(),
            accepts: accepts.iter().map(|s| s.to_string()).collect(),
            produces: produces.iter().map(|s| s.to_string()).collect(),
            may_produce: may_produce.iter().map(|s| s.to_string()).collect(),
            trigger: TriggerCondition::OnSignal {
                name: "manual".into(),
            },
        }
    }

    // --- Pre-execution: enforce_preconditions ---

    #[test]
    fn preconditions_met() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc"]);
        store
            .record("doc", "a", Path::new("a.json"), &json!({"title": "A"}))
            .unwrap();

        let skill = make_skill("design", &["doc"], &[], &[], &[]);
        assert!(enforce_preconditions(&skill, &store).is_ok());
    }

    #[test]
    fn preconditions_missing_requires() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp.path().join("s"), vec!["doc"]);

        let skill = make_skill("design", &["doc"], &[], &[], &[]);
        let err = enforce_preconditions(&skill, &store).unwrap_err();
        assert_eq!(err.skill_name, "design");
        assert_eq!(err.phase, Phase::Pre);
        assert_eq!(err.failures.len(), 1);
        assert!(matches!(
            &err.failures[0],
            ArtifactFailure::Missing { artifact_type, relationship: Relationship::Requires }
            if artifact_type == "doc"
        ));
    }

    #[test]
    fn preconditions_invalid_requires() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc"]);
        store
            .record("doc", "bad", Path::new("b.json"), &json!({"bad": true}))
            .unwrap();

        let skill = make_skill("design", &["doc"], &[], &[], &[]);
        let err = enforce_preconditions(&skill, &store).unwrap_err();
        assert_eq!(err.failures.len(), 1);
        assert!(matches!(
            &err.failures[0],
            ArtifactFailure::Invalid { artifact_type, relationship: Relationship::Requires, violations }
            if artifact_type == "doc" && !violations.is_empty()
        ));
    }

    #[test]
    fn preconditions_stale_requires() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc"]);
        store
            .record("doc", "a", Path::new("a.json"), &json!({"title": "A"}))
            .unwrap();
        store.invalidate("doc", "a").unwrap();

        let skill = make_skill("design", &["doc"], &[], &[], &[]);
        let err = enforce_preconditions(&skill, &store).unwrap_err();
        assert_eq!(err.failures.len(), 1);
        assert!(matches!(
            &err.failures[0],
            ArtifactFailure::Stale { artifact_type, relationship: Relationship::Requires, stale_count: 1 }
            if artifact_type == "doc"
        ));
    }

    #[test]
    fn preconditions_empty_requires() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp.path().join("s"), vec!["doc"]);

        let skill = make_skill("design", &[], &[], &[], &[]);
        assert!(enforce_preconditions(&skill, &store).is_ok());
    }

    #[test]
    fn preconditions_ignores_accepts() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp.path().join("s"), vec!["doc", "notes"]);

        // "notes" is in accepts and is missing — should not cause failure.
        let skill = make_skill("design", &[], &["notes"], &[], &[]);
        assert!(enforce_preconditions(&skill, &store).is_ok());
    }

    #[test]
    fn preconditions_collects_all_failures() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp.path().join("s"), vec!["doc", "spec"]);

        let skill = make_skill("design", &["doc", "spec"], &[], &[], &[]);
        let err = enforce_preconditions(&skill, &store).unwrap_err();
        assert_eq!(err.failures.len(), 2);
        // Both should be Missing.
        for failure in &err.failures {
            assert!(matches!(failure, ArtifactFailure::Missing { .. }));
        }
    }

    #[test]
    fn mixed_invalid_and_stale_instances() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc"]);
        // One invalid instance (missing required "title").
        store
            .record("doc", "bad", Path::new("b.json"), &json!({"bad": true}))
            .unwrap();
        // One valid instance, then mark it stale.
        store
            .record("doc", "ok", Path::new("ok.json"), &json!({"title": "ok"}))
            .unwrap();
        store.invalidate("doc", "ok").unwrap();

        let skill = make_skill("design", &["doc"], &[], &[], &[]);
        let err = enforce_preconditions(&skill, &store).unwrap_err();
        assert_eq!(err.failures.len(), 1);
        // Invalid takes precedence over Stale.
        assert!(matches!(
            &err.failures[0],
            ArtifactFailure::Invalid { artifact_type, relationship: Relationship::Requires, violations }
            if artifact_type == "doc" && !violations.is_empty()
        ));
    }

    // --- Post-execution: enforce_postconditions ---

    #[test]
    fn postconditions_met() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc"]);
        store
            .record("doc", "a", Path::new("a.json"), &json!({"title": "A"}))
            .unwrap();

        let skill = make_skill("design", &[], &[], &["doc"], &[]);
        assert!(enforce_postconditions(&skill, &store).is_ok());
    }

    #[test]
    fn postconditions_missing_produces() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp.path().join("s"), vec!["doc"]);

        let skill = make_skill("design", &[], &[], &["doc"], &[]);
        let err = enforce_postconditions(&skill, &store).unwrap_err();
        assert_eq!(err.phase, Phase::Post);
        assert_eq!(err.failures.len(), 1);
        assert!(matches!(
            &err.failures[0],
            ArtifactFailure::Missing { artifact_type, relationship: Relationship::Produces }
            if artifact_type == "doc"
        ));
    }

    #[test]
    fn postconditions_invalid_produces() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc"]);
        store
            .record("doc", "bad", Path::new("b.json"), &json!({"bad": true}))
            .unwrap();

        let skill = make_skill("design", &[], &[], &["doc"], &[]);
        let err = enforce_postconditions(&skill, &store).unwrap_err();
        assert_eq!(err.failures.len(), 1);
        assert!(matches!(
            &err.failures[0],
            ArtifactFailure::Invalid {
                relationship: Relationship::Produces,
                ..
            }
        ));
    }

    #[test]
    fn postconditions_stale_produces() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc"]);
        store
            .record("doc", "a", Path::new("a.json"), &json!({"title": "A"}))
            .unwrap();
        store.invalidate("doc", "a").unwrap();

        let skill = make_skill("design", &[], &[], &["doc"], &[]);
        let err = enforce_postconditions(&skill, &store).unwrap_err();
        assert_eq!(err.failures.len(), 1);
        assert!(matches!(
            &err.failures[0],
            ArtifactFailure::Stale { artifact_type, relationship: Relationship::Produces, stale_count: 1 }
            if artifact_type == "doc"
        ));
    }

    #[test]
    fn postconditions_may_produce_valid() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc", "notes"]);
        store
            .record("doc", "a", Path::new("a.json"), &json!({"title": "A"}))
            .unwrap();
        store
            .record("notes", "n1", Path::new("n.json"), &json!({"title": "N"}))
            .unwrap();

        let skill = make_skill("design", &[], &[], &["doc"], &["notes"]);
        assert!(enforce_postconditions(&skill, &store).is_ok());
    }

    #[test]
    fn postconditions_may_produce_invalid() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc", "notes"]);
        store
            .record("doc", "a", Path::new("a.json"), &json!({"title": "A"}))
            .unwrap();
        store
            .record("notes", "bad", Path::new("n.json"), &json!({"bad": true}))
            .unwrap();

        let skill = make_skill("design", &[], &[], &["doc"], &["notes"]);
        let err = enforce_postconditions(&skill, &store).unwrap_err();
        assert_eq!(err.failures.len(), 1);
        assert!(matches!(
            &err.failures[0],
            ArtifactFailure::Invalid { artifact_type, relationship: Relationship::MayProduce, .. }
            if artifact_type == "notes"
        ));
    }

    #[test]
    fn postconditions_may_produce_stale() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc", "notes"]);
        store
            .record("doc", "a", Path::new("a.json"), &json!({"title": "A"}))
            .unwrap();
        store
            .record("notes", "n1", Path::new("n.json"), &json!({"title": "N"}))
            .unwrap();
        store.invalidate("notes", "n1").unwrap();

        let skill = make_skill("design", &[], &[], &["doc"], &["notes"]);
        let err = enforce_postconditions(&skill, &store).unwrap_err();
        assert_eq!(err.failures.len(), 1);
        assert!(matches!(
            &err.failures[0],
            ArtifactFailure::Stale { artifact_type, relationship: Relationship::MayProduce, stale_count: 1 }
            if artifact_type == "notes"
        ));
    }

    #[test]
    fn postconditions_may_produce_absent() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc", "notes"]);
        store
            .record("doc", "a", Path::new("a.json"), &json!({"title": "A"}))
            .unwrap();

        // "notes" has no instances — may_produce should not fail.
        let skill = make_skill("design", &[], &[], &["doc"], &["notes"]);
        assert!(enforce_postconditions(&skill, &store).is_ok());
    }

    #[test]
    fn postconditions_empty() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp.path().join("s"), vec!["doc"]);

        let skill = make_skill("design", &[], &[], &[], &[]);
        assert!(enforce_postconditions(&skill, &store).is_ok());
    }

    #[test]
    fn postconditions_ignores_accepts() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc", "context"]);
        store
            .record("doc", "a", Path::new("a.json"), &json!({"title": "A"}))
            .unwrap();
        // "context" is in accepts and is missing — should not cause failure.

        let skill = make_skill("design", &[], &["context"], &["doc"], &[]);
        assert!(enforce_postconditions(&skill, &store).is_ok());
    }

    // --- Display formatting ---

    #[test]
    fn error_display_pre_missing() {
        let err = EnforcementError {
            skill_name: "design".into(),
            phase: Phase::Pre,
            failures: vec![ArtifactFailure::Missing {
                artifact_type: "constraints".into(),
                relationship: Relationship::Requires,
            }],
        };
        let msg = err.to_string();
        assert!(msg.contains("enforcement failed for skill 'design' (pre-execution):"));
        assert!(msg.contains("requires artifact type 'constraints' which is missing"));
    }

    #[test]
    fn error_display_post_missing() {
        let err = EnforcementError {
            skill_name: "design".into(),
            phase: Phase::Post,
            failures: vec![ArtifactFailure::Missing {
                artifact_type: "doc".into(),
                relationship: Relationship::Produces,
            }],
        };
        let msg = err.to_string();
        assert!(msg.contains("(post-execution):"));
        assert!(
            msg.contains(
                "declares it produces artifact type 'doc' which is missing after execution"
            )
        );
    }

    #[test]
    fn error_display_invalid_with_violations() {
        let err = EnforcementError {
            skill_name: "design".into(),
            phase: Phase::Pre,
            failures: vec![ArtifactFailure::Invalid {
                artifact_type: "doc".into(),
                relationship: Relationship::Requires,
                violations: vec![Violation {
                    artifact_type: "doc".into(),
                    description: "expected string, got integer".into(),
                    schema_path: "/properties/title/type".into(),
                    instance_path: "/title".into(),
                }],
            }],
        };
        let msg = err.to_string();
        assert!(msg.contains("requires artifact type 'doc' which exists but fails validation"));
        assert!(msg.contains("/properties/title/type: expected string, got integer"));
    }

    #[test]
    fn error_display_stale() {
        let err = EnforcementError {
            skill_name: "design".into(),
            phase: Phase::Pre,
            failures: vec![ArtifactFailure::Stale {
                artifact_type: "doc".into(),
                relationship: Relationship::Requires,
                stale_count: 2,
            }],
        };
        let msg = err.to_string();
        assert!(msg.contains("requires artifact type 'doc' which is stale"));
        assert!(msg.contains("2 instances need revalidation"));
    }
}
