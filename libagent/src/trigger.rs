use std::collections::{HashMap, HashSet};

use crate::model::TriggerCondition;
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
/// Bundles the artifact store with per-protocol completion timestamps and
/// currently active signals. Evaluation is pure — no side effects.
pub struct TriggerContext<'a> {
    pub store: &'a ArtifactStore,
    pub completion_timestamps: &'a HashMap<String, u64>,
    pub active_signals: &'a HashSet<String>,
    pub work_unit: Option<&'a str>,
}

/// Evaluate whether a trigger condition is satisfied given current state.
///
/// `protocol_name` identifies the protocol being evaluated, used to look up
/// its last completion timestamp for `OnChange` conditions.
pub fn evaluate(
    condition: &TriggerCondition,
    context: &TriggerContext<'_>,
    protocol_name: &str,
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
                    match context.completion_timestamps.get(protocol_name) {
                        None => {
                            // Never completed — any instance counts as changed.
                            TriggerResult::Satisfied
                        }
                        Some(&last_completion) => {
                            if latest > last_completion {
                                TriggerResult::Satisfied
                            } else {
                                TriggerResult::NotSatisfied(format!(
                                    "artifact type '{name}' has not changed since last completion"
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

        TriggerCondition::OnSignal { name } => {
            if context.active_signals.contains(name) {
                TriggerResult::Satisfied
            } else {
                TriggerResult::NotSatisfied(format!("signal '{name}' is not active"))
            }
        }

        TriggerCondition::AllOf { conditions } => {
            for child in conditions {
                let result = evaluate(child, context, protocol_name);
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
                match evaluate(child, context, protocol_name) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::make_store;
    use serde_json::json;
    use std::path::Path;
    use tempfile::TempDir;

    fn empty_context(store: &ArtifactStore) -> TriggerContext<'_> {
        // Leaked to avoid lifetime issues in tests — fine for test code.
        TriggerContext {
            store,
            completion_timestamps: Box::leak(Box::default()),
            active_signals: Box::leak(Box::default()),
            work_unit: None,
        }
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
    fn on_change_satisfied_when_never_completed() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc"]);
        store
            .record_with_timestamp(
                "doc",
                "a",
                Path::new("a.json"),
                &json!({"title": "A"}),
                1000,
            )
            .unwrap();

        let timestamps = HashMap::new();
        let signals = HashSet::new();
        let ctx = TriggerContext {
            store: &store,
            completion_timestamps: &timestamps,
            active_signals: &signals,
            work_unit: None,
        };
        let cond = TriggerCondition::OnChange { name: "doc".into() };
        assert_eq!(evaluate(&cond, &ctx, "protocol"), TriggerResult::Satisfied);
    }

    #[test]
    fn on_change_satisfied_when_modified_after_completion() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc"]);
        store
            .record_with_timestamp(
                "doc",
                "a",
                Path::new("a.json"),
                &json!({"title": "A"}),
                2000,
            )
            .unwrap();

        let timestamps = HashMap::from([("protocol".to_string(), 1000u64)]);
        let signals = HashSet::new();
        let ctx = TriggerContext {
            store: &store,
            completion_timestamps: &timestamps,
            active_signals: &signals,
            work_unit: None,
        };
        let cond = TriggerCondition::OnChange { name: "doc".into() };
        assert_eq!(evaluate(&cond, &ctx, "protocol"), TriggerResult::Satisfied);
    }

    #[test]
    fn on_change_not_satisfied_when_not_modified_since_completion() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc"]);
        store
            .record_with_timestamp(
                "doc",
                "a",
                Path::new("a.json"),
                &json!({"title": "A"}),
                1000,
            )
            .unwrap();

        let timestamps = HashMap::from([("protocol".to_string(), 2000u64)]);
        let signals = HashSet::new();
        let ctx = TriggerContext {
            store: &store,
            completion_timestamps: &timestamps,
            active_signals: &signals,
            work_unit: None,
        };
        let cond = TriggerCondition::OnChange { name: "doc".into() };
        assert!(matches!(
            evaluate(&cond, &ctx, "protocol"),
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
        let mut store = make_store(&tmp.path().join("s"), vec!["doc"]);
        store
            .record_with_timestamp(
                "doc",
                "a",
                Path::new("a.json"),
                &json!({"title": "A"}),
                1000,
            )
            .unwrap();

        let timestamps = HashMap::from([("protocol".to_string(), 1000u64)]);
        let signals = HashSet::new();
        let ctx = TriggerContext {
            store: &store,
            completion_timestamps: &timestamps,
            active_signals: &signals,
            work_unit: None,
        };
        let cond = TriggerCondition::OnChange { name: "doc".into() };
        assert!(matches!(
            evaluate(&cond, &ctx, "protocol"),
            TriggerResult::NotSatisfied(_)
        ));
    }

    #[test]
    fn on_change_satisfied_when_one_of_multiple_instances_newer() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc"]);
        // Old instance — before completion.
        store
            .record_with_timestamp(
                "doc",
                "old",
                Path::new("old.json"),
                &json!({"title": "old"}),
                500,
            )
            .unwrap();
        // New instance — after completion.
        store
            .record_with_timestamp(
                "doc",
                "new",
                Path::new("new.json"),
                &json!({"title": "new"}),
                2000,
            )
            .unwrap();

        let timestamps = HashMap::from([("protocol".to_string(), 1000u64)]);
        let signals = HashSet::new();
        let ctx = TriggerContext {
            store: &store,
            completion_timestamps: &timestamps,
            active_signals: &signals,
            work_unit: None,
        };
        let cond = TriggerCondition::OnChange { name: "doc".into() };
        // Any single instance newer than completion → satisfied.
        assert_eq!(evaluate(&cond, &ctx, "protocol"), TriggerResult::Satisfied);
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

    // --- OnSignal ---

    #[test]
    fn on_signal_satisfied_when_active() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp.path().join("s"), vec!["doc"]);

        let timestamps = HashMap::new();
        let signals = HashSet::from(["deploy".to_string()]);
        let ctx = TriggerContext {
            store: &store,
            completion_timestamps: &timestamps,
            active_signals: &signals,
            work_unit: None,
        };
        let cond = TriggerCondition::OnSignal {
            name: "deploy".into(),
        };
        assert_eq!(evaluate(&cond, &ctx, "protocol"), TriggerResult::Satisfied);
    }

    #[test]
    fn on_signal_not_satisfied_when_inactive() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp.path().join("s"), vec!["doc"]);

        let ctx = empty_context(&store);
        let cond = TriggerCondition::OnSignal {
            name: "deploy".into(),
        };
        assert!(matches!(
            evaluate(&cond, &ctx, "protocol"),
            TriggerResult::NotSatisfied(_)
        ));
    }

    // --- AllOf ---

    #[test]
    fn all_of_satisfied_when_all_children_satisfied() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc"]);
        store
            .record("doc", "a", Path::new("a.json"), &json!({"title": "A"}))
            .unwrap();

        let timestamps = HashMap::new();
        let signals = HashSet::from(["go".to_string()]);
        let ctx = TriggerContext {
            store: &store,
            completion_timestamps: &timestamps,
            active_signals: &signals,
            work_unit: None,
        };

        let cond = TriggerCondition::AllOf {
            conditions: vec![
                TriggerCondition::OnArtifact { name: "doc".into() },
                TriggerCondition::OnSignal { name: "go".into() },
            ],
        };
        assert_eq!(evaluate(&cond, &ctx, "protocol"), TriggerResult::Satisfied);
    }

    #[test]
    fn all_of_not_satisfied_when_one_child_fails() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc"]);
        store
            .record("doc", "a", Path::new("a.json"), &json!({"title": "A"}))
            .unwrap();

        let ctx = empty_context(&store);

        let cond = TriggerCondition::AllOf {
            conditions: vec![
                TriggerCondition::OnArtifact { name: "doc".into() },
                TriggerCondition::OnSignal {
                    name: "missing".into(),
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
        let store = make_store(&tmp.path().join("s"), vec!["doc"]);

        let timestamps = HashMap::new();
        let signals = HashSet::from(["go".to_string()]);
        let ctx = TriggerContext {
            store: &store,
            completion_timestamps: &timestamps,
            active_signals: &signals,
            work_unit: None,
        };

        let cond = TriggerCondition::AnyOf {
            conditions: vec![
                TriggerCondition::OnArtifact { name: "doc".into() },
                TriggerCondition::OnSignal { name: "go".into() },
            ],
        };
        assert_eq!(evaluate(&cond, &ctx, "protocol"), TriggerResult::Satisfied);
    }

    #[test]
    fn any_of_not_satisfied_when_all_children_fail() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp.path().join("s"), vec!["doc"]);
        let ctx = empty_context(&store);

        let cond = TriggerCondition::AnyOf {
            conditions: vec![
                TriggerCondition::OnArtifact { name: "doc".into() },
                TriggerCondition::OnSignal {
                    name: "missing".into(),
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
        let mut store = make_store(&tmp.path().join("s"), vec!["doc", "approval"]);
        store
            .record("doc", "a", Path::new("a.json"), &json!({"title": "A"}))
            .unwrap();
        // "approval" has no instances — OnArtifact for it would fail.

        let timestamps = HashMap::new();
        let signals = HashSet::from(["approved".to_string()]);
        let ctx = TriggerContext {
            store: &store,
            completion_timestamps: &timestamps,
            active_signals: &signals,
            work_unit: None,
        };

        // AllOf(OnArtifact("doc"), AnyOf(OnSignal("approved"), OnArtifact("approval")))
        // doc is valid, signal "approved" is active → satisfied.
        let cond = TriggerCondition::AllOf {
            conditions: vec![
                TriggerCondition::OnArtifact { name: "doc".into() },
                TriggerCondition::AnyOf {
                    conditions: vec![
                        TriggerCondition::OnSignal {
                            name: "approved".into(),
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
        let mut store = make_store(&tmp.path().join("s"), vec!["doc", "approval"]);
        store
            .record("doc", "a", Path::new("a.json"), &json!({"title": "A"}))
            .unwrap();

        let ctx = empty_context(&store);

        // AllOf(OnArtifact("doc"), AnyOf(OnSignal("approved"), OnArtifact("approval")))
        // doc is valid but neither signal nor approval artifact → not satisfied.
        let cond = TriggerCondition::AllOf {
            conditions: vec![
                TriggerCondition::OnArtifact { name: "doc".into() },
                TriggerCondition::AnyOf {
                    conditions: vec![
                        TriggerCondition::OnSignal {
                            name: "approved".into(),
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

        let timestamps = HashMap::new();
        let signals = HashSet::new();

        // Scoped to WU-A: only valid instance visible → satisfied.
        let ctx_a = TriggerContext {
            store: &store,
            completion_timestamps: &timestamps,
            active_signals: &signals,
            work_unit: Some("wu-a"),
        };
        let cond = TriggerCondition::OnArtifact { name: "doc".into() };
        assert_eq!(
            evaluate(&cond, &ctx_a, "protocol"),
            TriggerResult::Satisfied
        );

        // Scoped to WU-B: only invalid instance visible → not satisfied.
        let ctx_b = TriggerContext {
            store: &store,
            completion_timestamps: &timestamps,
            active_signals: &signals,
            work_unit: Some("wu-b"),
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

        let timestamps = HashMap::new();
        let signals = HashSet::new();
        let ctx = TriggerContext {
            store: &store,
            completion_timestamps: &timestamps,
            active_signals: &signals,
            work_unit: Some("wu-x"),
        };
        let cond = TriggerCondition::OnArtifact { name: "doc".into() };
        assert_eq!(evaluate(&cond, &ctx, "protocol"), TriggerResult::Satisfied);
    }
}
