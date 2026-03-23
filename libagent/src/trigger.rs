use std::collections::HashSet;

use crate::enforcement::enforce_postconditions;
use crate::model::{ProtocolDeclaration, TriggerCondition};
use crate::store::{ArtifactStore, ValidationStatus};

/// Outcome of evaluating a trigger condition against current state.
///
/// `NotSatisfied` is a normal outcome, not an error — a protocol simply
/// doesn't activate yet.
#[derive(Debug, Clone, PartialEq)]
pub enum TriggerResult {
    /// The condition is met; the protocol should activate.
    Satisfied,
    /// The condition is not met; includes a human-readable reason.
    NotSatisfied(String),
}

/// Read-only snapshot of the state needed to evaluate trigger conditions.
///
/// Bundles the artifact store with scan metadata. Evaluation is pure — no
/// side effects.
pub struct TriggerContext<'a> {
    pub store: &'a ArtifactStore,
    pub work_unit: Option<&'a str>,
    pub partially_scanned_types: &'a HashSet<String>,
}

/// Evaluate whether a trigger condition is satisfied given current state.
pub fn evaluate(
    condition: &TriggerCondition,
    protocol: &ProtocolDeclaration,
    context: &TriggerContext<'_>,
) -> TriggerResult {
    match condition {
        TriggerCondition::OnArtifact { name } => {
            if context.store.is_valid(name, context.work_unit) {
                TriggerResult::Satisfied
            } else {
                let instances = context.store.instances_of(name, context.work_unit);
                if instances.is_empty() {
                    TriggerResult::NotSatisfied(format!(
                        "no valid instances of artifact type '{name}' exist"
                    ))
                } else if instances.iter().any(|(_, state)| {
                    matches!(
                        state.status,
                        ValidationStatus::Invalid(_)
                            | ValidationStatus::Malformed(_)
                            | ValidationStatus::Stale
                    )
                }) {
                    TriggerResult::NotSatisfied(format!(
                        "artifact type '{name}' has invalid, malformed, or stale instances"
                    ))
                } else {
                    TriggerResult::NotSatisfied(format!("artifact type '{name}' is not valid"))
                }
            }
        }

        TriggerCondition::OnChange { name } => {
            match context
                .store
                .latest_modification_ms(name, context.work_unit)
            {
                None => TriggerResult::NotSatisfied(format!(
                    "no instances of artifact type '{name}' exist"
                )),
                Some(latest) => {
                    match derived_completion_timestamp(
                        protocol,
                        context.store,
                        context.work_unit,
                        context.partially_scanned_types,
                    ) {
                        None => {
                            // No output evidence — any input instance counts as changed.
                            TriggerResult::Satisfied
                        }
                        Some(last_output_update) => {
                            if latest > last_output_update {
                                TriggerResult::Satisfied
                            } else {
                                TriggerResult::NotSatisfied(format!(
                                    "artifact type '{name}' has not changed since protocol outputs were last updated"
                                ))
                            }
                        }
                    }
                }
            }
        }

        TriggerCondition::OnInvalid { name } => {
            if context.store.has_any_invalid(name, context.work_unit) {
                TriggerResult::Satisfied
            } else {
                TriggerResult::NotSatisfied(format!(
                    "no invalid instances of artifact type '{name}'"
                ))
            }
        }

        TriggerCondition::AllOf { conditions } => {
            for child in conditions {
                let result = evaluate(child, protocol, context);
                if let TriggerResult::NotSatisfied(_) = result {
                    return result;
                }
            }
            TriggerResult::Satisfied
        }

        TriggerCondition::AnyOf { conditions } => {
            if conditions.is_empty() {
                return TriggerResult::NotSatisfied("any_of with no conditions".to_string());
            }
            let mut reasons = Vec::new();
            for child in conditions {
                match evaluate(child, protocol, context) {
                    TriggerResult::Satisfied => return TriggerResult::Satisfied,
                    TriggerResult::NotSatisfied(reason) => {
                        reasons.push(reason);
                    }
                }
            }
            TriggerResult::NotSatisfied(format!("none satisfied: {}", reasons.join("; ")))
        }
    }
}

pub(crate) fn derived_completion_timestamp(
    protocol: &ProtocolDeclaration,
    store: &ArtifactStore,
    work_unit: Option<&str>,
    partially_scanned_types: &HashSet<String>,
) -> Option<u64> {
    if protocol.produces.is_empty() {
        return None;
    }

    if protocol.produces.iter().any(|artifact_type| {
        store.scan_gap_affects_work_unit(artifact_type, work_unit)
            || (partially_scanned_types.contains(artifact_type.as_str())
                && !store.has_any_scan_gap_for_type(artifact_type))
    }) {
        return None;
    }

    if enforce_postconditions(protocol, store, work_unit).is_err() {
        return None;
    }

    protocol
        .produces
        .iter()
        .filter_map(|artifact_type| {
            store
                .instances_of(artifact_type, work_unit)
                .into_iter()
                .map(|(_, state)| state.last_modified_ms)
                .max()
        })
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ProtocolDeclaration;
    use crate::test_helpers::make_store;
    use serde_json::json;
    use std::path::Path;
    use tempfile::TempDir;

    fn make_protocol(
        trigger: TriggerCondition,
        produces: &[&str],
        may_produce: &[&str],
    ) -> ProtocolDeclaration {
        ProtocolDeclaration {
            name: "protocol".into(),
            requires: Vec::new(),
            accepts: Vec::new(),
            produces: produces.iter().map(|s| s.to_string()).collect(),
            may_produce: may_produce.iter().map(|s| s.to_string()).collect(),
            trigger,
        }
    }

    fn empty_context(store: &ArtifactStore) -> TriggerContext<'_> {
        // Leaked to avoid lifetime issues in tests — fine for test code.
        TriggerContext {
            store,
            work_unit: None,
            partially_scanned_types: Box::leak(Box::default()),
        }
    }

    fn empty_partials() -> &'static HashSet<String> {
        Box::leak(Box::default())
    }

    fn evaluate(
        condition: &TriggerCondition,
        context: &TriggerContext<'_>,
        _name: &str,
    ) -> TriggerResult {
        let protocol = make_protocol(condition.clone(), &[], &[]);
        super::evaluate(condition, &protocol, context)
    }

    // --- OnArtifact ---

    #[test]
    fn on_artifact_satisfied_when_all_valid() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc"]);
        store
            .record("doc", "a", Path::new("a.json"), &json!({"title": "A"}))
            .unwrap();

        let ctx = empty_context(&store);
        let cond = TriggerCondition::OnArtifact { name: "doc".into() };
        assert_eq!(evaluate(&cond, &ctx, "protocol"), TriggerResult::Satisfied);
    }

    #[test]
    fn on_artifact_not_satisfied_when_invalid_instance() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc"]);
        store
            .record("doc", "a", Path::new("a.json"), &json!({"title": "A"}))
            .unwrap();
        store
            .record("doc", "b", Path::new("b.json"), &json!({"bad": true}))
            .unwrap();

        let ctx = empty_context(&store);
        let cond = TriggerCondition::OnArtifact { name: "doc".into() };
        assert!(matches!(
            evaluate(&cond, &ctx, "protocol"),
            TriggerResult::NotSatisfied(_)
        ));
    }

    #[test]
    fn on_artifact_mentions_malformed_instances_in_failure_reason() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc"]);
        store
            .record_malformed(
                "doc",
                "bad",
                Path::new("bad.json"),
                br#"{"title":"A""#,
                "eof",
            )
            .unwrap();

        let ctx = empty_context(&store);
        let cond = TriggerCondition::OnArtifact { name: "doc".into() };
        assert_eq!(
            evaluate(&cond, &ctx, "protocol"),
            TriggerResult::NotSatisfied(
                "artifact type 'doc' has invalid, malformed, or stale instances".into()
            )
        );
    }

    #[test]
    fn on_artifact_not_satisfied_when_no_instances() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp.path().join("s"), vec!["doc"]);

        let ctx = empty_context(&store);
        let cond = TriggerCondition::OnArtifact { name: "doc".into() };
        assert!(matches!(
            evaluate(&cond, &ctx, "protocol"),
            TriggerResult::NotSatisfied(_)
        ));
    }

    #[test]
    fn on_artifact_not_satisfied_when_stale() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc"]);
        store
            .record("doc", "a", Path::new("a.json"), &json!({"title": "A"}))
            .unwrap();
        store.invalidate("doc", "a").unwrap();

        let ctx = empty_context(&store);
        let cond = TriggerCondition::OnArtifact { name: "doc".into() };
        assert!(matches!(
            evaluate(&cond, &ctx, "protocol"),
            TriggerResult::NotSatisfied(_)
        ));
    }

    // --- OnChange ---

    #[test]
    fn on_change_satisfied_when_no_output_evidence() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc", "implementation"]);
        store
            .record_with_timestamp(
                "doc",
                "a",
                Path::new("a.json"),
                &json!({"title": "A"}),
                1000,
            )
            .unwrap();

        let ctx = TriggerContext {
            store: &store,
            work_unit: None,
            partially_scanned_types: empty_partials(),
        };
        let cond = TriggerCondition::OnChange { name: "doc".into() };
        let protocol = make_protocol(cond.clone(), &["implementation"], &[]);
        assert_eq!(
            super::evaluate(&cond, &protocol, &ctx),
            TriggerResult::Satisfied
        );
    }

    #[test]
    fn on_change_satisfied_when_input_newer_than_output() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc", "implementation"]);
        store
            .record_with_timestamp(
                "doc",
                "a",
                Path::new("a.json"),
                &json!({"title": "A"}),
                2000,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "implementation",
                "a",
                Path::new("impl.json"),
                &json!({"title": "done"}),
                1000,
            )
            .unwrap();

        let ctx = TriggerContext {
            store: &store,
            work_unit: None,
            partially_scanned_types: empty_partials(),
        };
        let cond = TriggerCondition::OnChange { name: "doc".into() };
        let protocol = make_protocol(cond.clone(), &["implementation"], &[]);
        assert_eq!(
            super::evaluate(&cond, &protocol, &ctx),
            TriggerResult::Satisfied
        );
    }

    #[test]
    fn on_change_not_satisfied_when_output_newer_than_input() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc", "implementation"]);
        store
            .record_with_timestamp(
                "doc",
                "a",
                Path::new("a.json"),
                &json!({"title": "A"}),
                1000,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "implementation",
                "a",
                Path::new("impl.json"),
                &json!({"title": "done"}),
                2000,
            )
            .unwrap();

        let ctx = TriggerContext {
            store: &store,
            work_unit: None,
            partially_scanned_types: empty_partials(),
        };
        let cond = TriggerCondition::OnChange { name: "doc".into() };
        let protocol = make_protocol(cond.clone(), &["implementation"], &[]);
        assert!(matches!(
            super::evaluate(&cond, &protocol, &ctx),
            TriggerResult::NotSatisfied(_)
        ));
    }

    #[test]
    fn on_change_not_satisfied_when_no_instances() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp.path().join("s"), vec!["doc"]);

        let ctx = empty_context(&store);
        let cond = TriggerCondition::OnChange { name: "doc".into() };
        assert!(matches!(
            evaluate(&cond, &ctx, "protocol"),
            TriggerResult::NotSatisfied(_)
        ));
    }

    #[test]
    fn on_change_not_satisfied_when_same_timestamp() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc", "implementation"]);
        store
            .record_with_timestamp(
                "doc",
                "a",
                Path::new("a.json"),
                &json!({"title": "A"}),
                1000,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "implementation",
                "a",
                Path::new("impl.json"),
                &json!({"title": "done"}),
                1000,
            )
            .unwrap();

        let ctx = TriggerContext {
            store: &store,
            work_unit: None,
            partially_scanned_types: empty_partials(),
        };
        let cond = TriggerCondition::OnChange { name: "doc".into() };
        let protocol = make_protocol(cond.clone(), &["implementation"], &[]);
        assert!(matches!(
            super::evaluate(&cond, &protocol, &ctx),
            TriggerResult::NotSatisfied(_)
        ));
    }

    #[test]
    fn on_change_uses_newest_matching_output_timestamp_per_type() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc", "implementation"]);
        store
            .record_with_timestamp(
                "doc",
                "a",
                Path::new("a.json"),
                &json!({"title": "new"}),
                1500,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "implementation",
                "old-output",
                Path::new("old-output.json"),
                &json!({"title": "old"}),
                1000,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "implementation",
                "new-output",
                Path::new("new-output.json"),
                &json!({"title": "new"}),
                2500,
            )
            .unwrap();

        let ctx = TriggerContext {
            store: &store,
            work_unit: None,
            partially_scanned_types: empty_partials(),
        };
        let cond = TriggerCondition::OnChange { name: "doc".into() };
        let protocol = make_protocol(cond.clone(), &["implementation"], &[]);
        assert!(matches!(
            super::evaluate(&cond, &protocol, &ctx),
            TriggerResult::NotSatisfied(_)
        ));
    }

    #[test]
    fn on_change_ignores_stale_may_produce_timestamps() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(
            &tmp.path().join("s"),
            vec!["doc", "implementation", "notes"],
        );
        store
            .record_with_timestamp(
                "doc",
                "a",
                Path::new("a.json"),
                &json!({"title": "current"}),
                1500,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "implementation",
                "impl",
                Path::new("impl.json"),
                &json!({"title": "done"}),
                2000,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "notes",
                "old-note",
                Path::new("notes.json"),
                &json!({"title": "old"}),
                500,
            )
            .unwrap();

        let ctx = TriggerContext {
            store: &store,
            work_unit: None,
            partially_scanned_types: empty_partials(),
        };
        let cond = TriggerCondition::OnChange { name: "doc".into() };
        let protocol = make_protocol(cond.clone(), &["implementation"], &["notes"]);
        assert!(matches!(
            super::evaluate(&cond, &protocol, &ctx),
            TriggerResult::NotSatisfied(_)
        ));
    }

    #[test]
    fn derived_completion_timestamp_is_none_for_may_produce_only_protocols() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["notes"]);
        store
            .record_with_timestamp(
                "notes",
                "note",
                Path::new("note.json"),
                &json!({"title": "optional"}),
                1000,
            )
            .unwrap();

        let protocol = make_protocol(
            TriggerCondition::OnChange {
                name: "unused".into(),
            },
            &[],
            &["notes"],
        );
        assert_eq!(
            derived_completion_timestamp(&protocol, &store, None, &HashSet::new()),
            None
        );
    }

    #[test]
    fn derived_completion_timestamp_is_none_when_produces_type_is_partially_scanned() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["implementation"]);
        store
            .record_with_timestamp(
                "implementation",
                "impl",
                Path::new("impl.json"),
                &json!({"title": "done"}),
                1000,
            )
            .unwrap();

        let partial = HashSet::from(["implementation".to_string()]);
        let protocol = make_protocol(
            TriggerCondition::OnChange {
                name: "unused".into(),
            },
            &["implementation"],
            &[],
        );

        assert_eq!(
            derived_completion_timestamp(&protocol, &store, None, &partial),
            None
        );
    }

    #[test]
    fn derived_completion_timestamp_scopes_instance_scan_gaps_by_work_unit() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["implementation"]);
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let impl_a = workspace.join("impl-a.json");
        let impl_b = workspace.join("impl-b.json");
        std::fs::write(&impl_a, r#"{"title":"done-a","work_unit":"wu-a"}"#).unwrap();
        std::fs::write(&impl_b, r#"{"title":"done-b","work_unit":"wu-b"}"#).unwrap();
        store
            .record_with_timestamp(
                "implementation",
                "a",
                &impl_a,
                &json!({"title": "done-a", "work_unit": "wu-a"}),
                1000,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "implementation",
                "b",
                &impl_b,
                &json!({"title": "done-b", "work_unit": "wu-b"}),
                2000,
            )
            .unwrap();
        let observed_mtime = store
            .get("implementation", "a")
            .unwrap()
            .source_last_modified_ms;
        store.mark_instance_scan_gap("implementation", "a", observed_mtime);

        let partial = HashSet::from(["implementation".to_string()]);
        let protocol = make_protocol(
            TriggerCondition::OnChange {
                name: "unused".into(),
            },
            &["implementation"],
            &[],
        );

        assert_eq!(
            derived_completion_timestamp(&protocol, &store, Some("wu-a"), &partial),
            None
        );
        assert_eq!(
            derived_completion_timestamp(&protocol, &store, Some("wu-b"), &partial),
            Some(2000)
        );
    }

    #[test]
    fn derived_completion_timestamp_includes_unscoped_outputs_for_scoped_work_unit() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["implementation", "summary"]);
        store
            .record_with_timestamp(
                "summary",
                "shared",
                Path::new("shared.json"),
                &json!({"title": "shared"}),
                1000,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "implementation",
                "wu-a",
                Path::new("wu-a.json"),
                &json!({"title": "scoped", "work_unit": "wu-a"}),
                2000,
            )
            .unwrap();

        let protocol = make_protocol(
            TriggerCondition::OnChange {
                name: "unused".into(),
            },
            &["implementation", "summary"],
            &[],
        );

        assert_eq!(
            derived_completion_timestamp(&protocol, &store, Some("wu-a"), &HashSet::new()),
            Some(1000)
        );
    }

    #[test]
    fn derived_completion_timestamp_uses_newest_unscoped_output_for_unscoped_work() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["implementation"]);
        store
            .record_with_timestamp(
                "implementation",
                "shared-old",
                Path::new("shared-old.json"),
                &json!({"title": "old"}),
                1000,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "implementation",
                "shared-new",
                Path::new("shared-new.json"),
                &json!({"title": "new"}),
                2000,
            )
            .unwrap();

        let protocol = make_protocol(
            TriggerCondition::OnChange {
                name: "unused".into(),
            },
            &["implementation"],
            &[],
        );

        assert_eq!(
            derived_completion_timestamp(&protocol, &store, None, &HashSet::new()),
            Some(2000)
        );
    }

    #[test]
    fn derived_completion_timestamp_uses_oldest_per_type_maximum() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["implementation", "review"]);
        store
            .record_with_timestamp(
                "implementation",
                "impl-old",
                Path::new("impl-old.json"),
                &json!({"title": "old"}),
                1000,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "implementation",
                "impl-new",
                Path::new("impl-new.json"),
                &json!({"title": "new"}),
                2500,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "review",
                "review-current",
                Path::new("review.json"),
                &json!({"title": "review"}),
                2000,
            )
            .unwrap();

        let protocol = make_protocol(
            TriggerCondition::OnChange {
                name: "unused".into(),
            },
            &["implementation", "review"],
            &[],
        );

        assert_eq!(
            derived_completion_timestamp(&protocol, &store, None, &HashSet::new()),
            Some(2000)
        );
    }

    #[test]
    fn on_change_satisfied_when_required_outputs_are_partially_scanned() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc", "implementation"]);
        store
            .record_with_timestamp(
                "doc",
                "a",
                Path::new("a.json"),
                &json!({"title": "A"}),
                1000,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "implementation",
                "a",
                Path::new("impl.json"),
                &json!({"title": "done"}),
                2000,
            )
            .unwrap();

        let partial = HashSet::from(["implementation".to_string()]);
        let ctx = TriggerContext {
            store: &store,
            work_unit: None,
            partially_scanned_types: &partial,
        };
        let cond = TriggerCondition::OnChange { name: "doc".into() };
        let protocol = make_protocol(cond.clone(), &["implementation"], &[]);
        assert_eq!(
            super::evaluate(&cond, &protocol, &ctx),
            TriggerResult::Satisfied
        );
    }

    // --- OnInvalid ---

    #[test]
    fn on_invalid_satisfied_with_invalid_instance() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc"]);
        store
            .record("doc", "bad", Path::new("b.json"), &json!({"bad": true}))
            .unwrap();

        let ctx = empty_context(&store);
        let cond = TriggerCondition::OnInvalid { name: "doc".into() };
        assert_eq!(evaluate(&cond, &ctx, "protocol"), TriggerResult::Satisfied);
    }

    #[test]
    fn on_invalid_satisfied_with_malformed_instance() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc"]);
        store
            .record_malformed(
                "doc",
                "bad",
                Path::new("b.json"),
                b"not json",
                "expected value",
            )
            .unwrap();

        let ctx = empty_context(&store);
        let cond = TriggerCondition::OnInvalid { name: "doc".into() };
        assert_eq!(evaluate(&cond, &ctx, "protocol"), TriggerResult::Satisfied);
    }

    #[test]
    fn on_invalid_not_satisfied_when_all_valid() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc"]);
        store
            .record("doc", "a", Path::new("a.json"), &json!({"title": "A"}))
            .unwrap();

        let ctx = empty_context(&store);
        let cond = TriggerCondition::OnInvalid { name: "doc".into() };
        assert!(matches!(
            evaluate(&cond, &ctx, "protocol"),
            TriggerResult::NotSatisfied(_)
        ));
    }

    #[test]
    fn on_invalid_not_satisfied_when_no_instances() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp.path().join("s"), vec!["doc"]);

        let ctx = empty_context(&store);
        let cond = TriggerCondition::OnInvalid { name: "doc".into() };
        assert!(matches!(
            evaluate(&cond, &ctx, "protocol"),
            TriggerResult::NotSatisfied(_)
        ));
    }

    #[test]
    fn on_invalid_not_satisfied_when_stale() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc"]);
        store
            .record("doc", "a", Path::new("a.json"), &json!({"title": "A"}))
            .unwrap();
        store.invalidate("doc", "a").unwrap();

        let ctx = empty_context(&store);
        let cond = TriggerCondition::OnInvalid { name: "doc".into() };
        // Stale is not Invalid — should not satisfy on_invalid.
        assert!(matches!(
            evaluate(&cond, &ctx, "protocol"),
            TriggerResult::NotSatisfied(_)
        ));
    }

    // --- AllOf ---

    #[test]
    fn all_of_satisfied_when_all_children_satisfied() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc", "review"]);
        store
            .record("doc", "a", Path::new("a.json"), &json!({"title": "A"}))
            .unwrap();
        store
            .record("review", "a", Path::new("r.json"), &json!({"title": "R"}))
            .unwrap();

        let ctx = empty_context(&store);

        let cond = TriggerCondition::AllOf {
            conditions: vec![
                TriggerCondition::OnArtifact { name: "doc".into() },
                TriggerCondition::OnArtifact {
                    name: "review".into(),
                },
            ],
        };
        assert_eq!(evaluate(&cond, &ctx, "protocol"), TriggerResult::Satisfied);
    }

    #[test]
    fn all_of_not_satisfied_when_one_child_fails() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc", "review"]);
        store
            .record("doc", "a", Path::new("a.json"), &json!({"title": "A"}))
            .unwrap();

        let ctx = empty_context(&store);

        let cond = TriggerCondition::AllOf {
            conditions: vec![
                TriggerCondition::OnArtifact { name: "doc".into() },
                TriggerCondition::OnArtifact {
                    name: "review".into(),
                },
            ],
        };
        assert!(matches!(
            evaluate(&cond, &ctx, "protocol"),
            TriggerResult::NotSatisfied(_)
        ));
    }

    #[test]
    fn all_of_empty_is_satisfied() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp.path().join("s"), vec!["doc"]);
        let ctx = empty_context(&store);

        let cond = TriggerCondition::AllOf { conditions: vec![] };
        assert_eq!(evaluate(&cond, &ctx, "protocol"), TriggerResult::Satisfied);
    }

    // --- AnyOf ---

    #[test]
    fn any_of_satisfied_when_one_child_satisfied() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc", "review"]);
        store
            .record("doc", "a", Path::new("a.json"), &json!({"title": "A"}))
            .unwrap();

        let ctx = empty_context(&store);

        let cond = TriggerCondition::AnyOf {
            conditions: vec![
                TriggerCondition::OnArtifact { name: "doc".into() },
                TriggerCondition::OnArtifact {
                    name: "review".into(),
                },
            ],
        };
        assert_eq!(evaluate(&cond, &ctx, "protocol"), TriggerResult::Satisfied);
    }

    #[test]
    fn any_of_not_satisfied_when_all_children_fail() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp.path().join("s"), vec!["doc", "review"]);
        let ctx = empty_context(&store);

        let cond = TriggerCondition::AnyOf {
            conditions: vec![
                TriggerCondition::OnArtifact { name: "doc".into() },
                TriggerCondition::OnArtifact {
                    name: "review".into(),
                },
            ],
        };
        assert!(matches!(
            evaluate(&cond, &ctx, "protocol"),
            TriggerResult::NotSatisfied(_)
        ));
    }

    #[test]
    fn any_of_empty_is_not_satisfied() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp.path().join("s"), vec!["doc"]);
        let ctx = empty_context(&store);

        let cond = TriggerCondition::AnyOf { conditions: vec![] };
        assert!(matches!(
            evaluate(&cond, &ctx, "protocol"),
            TriggerResult::NotSatisfied(_)
        ));
    }

    // --- Nested composition ---

    #[test]
    fn nested_all_of_with_any_of() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc", "approval", "review"]);
        store
            .record("doc", "a", Path::new("a.json"), &json!({"title": "A"}))
            .unwrap();
        store
            .record("review", "a", Path::new("r.json"), &json!({"title": "R"}))
            .unwrap();
        // "approval" has no instances — OnArtifact for it would fail.

        let ctx = empty_context(&store);

        // AllOf(OnArtifact("doc"), AnyOf(OnArtifact("review"), OnArtifact("approval")))
        // doc is valid, review exists → satisfied.
        let cond = TriggerCondition::AllOf {
            conditions: vec![
                TriggerCondition::OnArtifact { name: "doc".into() },
                TriggerCondition::AnyOf {
                    conditions: vec![
                        TriggerCondition::OnArtifact {
                            name: "review".into(),
                        },
                        TriggerCondition::OnArtifact {
                            name: "approval".into(),
                        },
                    ],
                },
            ],
        };
        assert_eq!(evaluate(&cond, &ctx, "protocol"), TriggerResult::Satisfied);
    }

    #[test]
    fn nested_all_of_with_any_of_not_satisfied() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc", "approval", "review"]);
        store
            .record("doc", "a", Path::new("a.json"), &json!({"title": "A"}))
            .unwrap();

        let ctx = empty_context(&store);

        // AllOf(OnArtifact("doc"), AnyOf(OnArtifact("review"), OnArtifact("approval")))
        // doc is valid but neither review nor approval exists → not satisfied.
        let cond = TriggerCondition::AllOf {
            conditions: vec![
                TriggerCondition::OnArtifact { name: "doc".into() },
                TriggerCondition::AnyOf {
                    conditions: vec![
                        TriggerCondition::OnArtifact {
                            name: "review".into(),
                        },
                        TriggerCondition::OnArtifact {
                            name: "approval".into(),
                        },
                    ],
                },
            ],
        };
        assert!(matches!(
            evaluate(&cond, &ctx, "protocol"),
            TriggerResult::NotSatisfied(_)
        ));
    }

    // --- Work unit scoping ---

    #[test]
    fn on_artifact_scoped() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc"]);
        store
            .record(
                "doc",
                "a",
                Path::new("a.json"),
                &json!({"title": "A", "work_unit": "wu-a"}),
            )
            .unwrap();
        store
            .record(
                "doc",
                "b",
                Path::new("b.json"),
                &json!({"bad": true, "work_unit": "wu-b"}),
            )
            .unwrap();

        // Scoped to WU-A: only valid instance visible → satisfied.
        let ctx_a = TriggerContext {
            store: &store,
            work_unit: Some("wu-a"),
            partially_scanned_types: empty_partials(),
        };
        let cond = TriggerCondition::OnArtifact { name: "doc".into() };
        assert_eq!(
            evaluate(&cond, &ctx_a, "protocol"),
            TriggerResult::Satisfied
        );

        // Scoped to WU-B: only invalid instance visible → not satisfied.
        let ctx_b = TriggerContext {
            store: &store,
            work_unit: Some("wu-b"),
            partially_scanned_types: empty_partials(),
        };
        assert!(matches!(
            evaluate(&cond, &ctx_b, "protocol"),
            TriggerResult::NotSatisfied(_)
        ));
    }

    #[test]
    fn unpartitioned_visible_to_scoped_trigger() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc"]);
        // Unpartitioned valid instance.
        store
            .record("doc", "shared", Path::new("s.json"), &json!({"title": "S"}))
            .unwrap();

        let ctx = TriggerContext {
            store: &store,
            work_unit: Some("wu-x"),
            partially_scanned_types: empty_partials(),
        };
        let cond = TriggerCondition::OnArtifact { name: "doc".into() };
        assert_eq!(evaluate(&cond, &ctx, "protocol"), TriggerResult::Satisfied);
    }
}
