use std::collections::{BTreeSet, HashSet};

use crate::activation::ActivationStore;
use crate::enforcement::enforce_postconditions;
use crate::model::{ProtocolDeclaration, TriggerCondition};
use crate::store::ArtifactStore;
use crate::trigger::{TriggerContext, TriggerResult, evaluate as evaluate_trigger};

/// A (protocol, work_unit) pair that is ready for execution.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub protocol_name: String,
    pub work_unit: Option<String>,
}

/// Discover all (protocol, work_unit) pairs that are ready for execution.
///
/// Evaluates protocols in topological order. For each protocol, discovers
/// candidate work_units from artifact instances referenced by the protocol's
/// edges and trigger tree, then evaluates readiness for each candidate.
///
/// Candidates are emitted in topological protocol order, with work_units
/// in deterministic lexicographic order within each protocol.
pub fn discover_ready_candidates(
    protocols: &[ProtocolDeclaration],
    store: &ArtifactStore,
    activations: &ActivationStore,
    active_signals: &HashSet<String>,
    topological_order: &[&str],
    partially_scanned_types: &HashSet<String>,
) -> Vec<Candidate> {
    let mut candidates = Vec::new();

    for &protocol_name in topological_order {
        let Some(protocol) = protocols.iter().find(|p| p.name == protocol_name) else {
            continue;
        };

        // Collect artifact type names from trigger tree (kept separate for
        // scan trust checks), then merge with requires and accepts.
        let mut trigger_types = HashSet::new();
        trigger_artifact_types(&protocol.trigger, &mut trigger_types);

        let mut referenced_types = HashSet::new();
        for name in &protocol.requires {
            referenced_types.insert(name.as_str());
        }
        for name in &protocol.accepts {
            referenced_types.insert(name.as_str());
        }
        for &t in &trigger_types {
            referenced_types.insert(t);
        }

        // Collect distinct work_unit values from referenced artifact instances.
        let work_units = collect_work_units(store, &referenced_types);

        for wu in &work_units {
            let wu_ref = wu.as_deref();

            // Scan trust: skip if any requires or trigger-referenced type
            // is partially scanned. Evaluating triggers or preconditions
            // against incomplete data could lead to false activation.
            if protocol
                .requires
                .iter()
                .any(|t| partially_scanned_types.contains(t.as_str()))
                || trigger_types
                    .iter()
                    .any(|t| partially_scanned_types.contains(*t))
            {
                continue;
            }

            // Build trigger context scoped to this work_unit.
            let timestamps = activations.timestamps_for_trigger_context(wu_ref);
            let ctx = TriggerContext {
                store,
                activation_timestamps: &timestamps,
                active_signals,
                work_unit: wu_ref,
            };

            // Evaluate trigger.
            if !matches!(
                evaluate_trigger(&protocol.trigger, &ctx, &protocol.name),
                TriggerResult::Satisfied
            ) {
                continue;
            }

            // Check preconditions.
            if crate::enforce_preconditions(protocol, store, wu_ref).is_err() {
                continue;
            }

            // Completed suppression: already activated and postconditions still pass.
            // Skip suppression when the trigger contains on_change — the trigger
            // was satisfied because inputs changed after the last activation, so
            // the protocol should re-run even if prior outputs are still valid.
            if activations.is_activated(&protocol.name, wu_ref)
                && enforce_postconditions(protocol, store, wu_ref).is_ok()
                && !any_on_change_satisfied(&protocol.trigger, &ctx, &protocol.name)
            {
                continue;
            }

            candidates.push(Candidate {
                protocol_name: protocol.name.clone(),
                work_unit: wu.clone(),
            });
        }
    }

    candidates
}

/// Walk a trigger condition tree and collect artifact type names
/// from `OnArtifact`, `OnChange`, and `OnInvalid` variants.
fn trigger_artifact_types<'a>(condition: &'a TriggerCondition, out: &mut HashSet<&'a str>) {
    match condition {
        TriggerCondition::OnArtifact { name }
        | TriggerCondition::OnChange { name }
        | TriggerCondition::OnInvalid { name } => {
            out.insert(name.as_str());
        }
        TriggerCondition::OnSignal { .. } => {}
        TriggerCondition::AllOf { conditions } | TriggerCondition::AnyOf { conditions } => {
            for child in conditions {
                trigger_artifact_types(child, out);
            }
        }
    }
}

/// True if any `OnChange` node in the trigger tree evaluates to Satisfied.
///
/// Walks the trigger tree and evaluates only `OnChange` conditions. Returns
/// true as soon as any one is Satisfied. For composite triggers like
/// `AnyOf(on_signal, on_change)`, this distinguishes whether the overall
/// Satisfied result came from a change-based branch or a non-change branch.
fn any_on_change_satisfied(
    condition: &TriggerCondition,
    context: &TriggerContext<'_>,
    protocol_name: &str,
) -> bool {
    match condition {
        TriggerCondition::OnChange { .. } => {
            matches!(
                evaluate_trigger(condition, context, protocol_name),
                TriggerResult::Satisfied
            )
        }
        TriggerCondition::AllOf { conditions } | TriggerCondition::AnyOf { conditions } => {
            conditions
                .iter()
                .any(|c| any_on_change_satisfied(c, context, protocol_name))
        }
        _ => false,
    }
}

/// Collect distinct work_unit values from artifact instances across multiple types.
///
/// Returns `BTreeSet` for deterministic lexicographic ordering. If no instances
/// reference any work_unit, returns `{None}` so the protocol is evaluated once
/// unscoped.
fn collect_work_units(
    store: &ArtifactStore,
    artifact_types: &HashSet<&str>,
) -> BTreeSet<Option<String>> {
    let mut work_units = BTreeSet::new();

    for &type_name in artifact_types {
        for (_, state) in store.instances_of(type_name, None) {
            work_units.insert(state.work_unit.clone());
        }
    }

    if work_units.is_empty() {
        work_units.insert(None);
    }

    work_units
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::make_store;
    use serde_json::json;
    use std::path::Path;
    use tempfile::TempDir;

    fn make_protocol(
        name: &str,
        requires: &[&str],
        accepts: &[&str],
        produces: &[&str],
        may_produce: &[&str],
        trigger: TriggerCondition,
    ) -> ProtocolDeclaration {
        ProtocolDeclaration {
            name: name.into(),
            requires: requires.iter().map(|s| s.to_string()).collect(),
            accepts: accepts.iter().map(|s| s.to_string()).collect(),
            produces: produces.iter().map(|s| s.to_string()).collect(),
            may_produce: may_produce.iter().map(|s| s.to_string()).collect(),
            trigger,
        }
    }

    #[test]
    fn trigger_artifact_types_collects_from_tree() {
        let trigger = TriggerCondition::AllOf {
            conditions: vec![
                TriggerCondition::OnArtifact {
                    name: "constraints".into(),
                },
                TriggerCondition::AnyOf {
                    conditions: vec![
                        TriggerCondition::OnChange {
                            name: "spec".into(),
                        },
                        TriggerCondition::OnInvalid {
                            name: "report".into(),
                        },
                        TriggerCondition::OnSignal {
                            name: "deploy".into(),
                        },
                    ],
                },
            ],
        };

        let mut types = HashSet::new();
        trigger_artifact_types(&trigger, &mut types);
        assert_eq!(types.len(), 3);
        assert!(types.contains("constraints"));
        assert!(types.contains("spec"));
        assert!(types.contains("report"));
    }

    #[test]
    fn collect_work_units_returns_none_when_no_instances() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp.path().join("s"), vec!["doc"]);

        let types = HashSet::from(["doc"]);
        let wus = collect_work_units(&store, &types);
        assert_eq!(wus, BTreeSet::from([None]));
    }

    #[test]
    fn collect_work_units_returns_distinct_values() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc"]);
        store
            .record(
                "doc",
                "a1",
                Path::new("a1.json"),
                &json!({"title": "A", "work_unit": "wu-a"}),
            )
            .unwrap();
        store
            .record(
                "doc",
                "b1",
                Path::new("b1.json"),
                &json!({"title": "B", "work_unit": "wu-b"}),
            )
            .unwrap();
        store
            .record(
                "doc",
                "shared",
                Path::new("shared.json"),
                &json!({"title": "S"}),
            )
            .unwrap();

        let types = HashSet::from(["doc"]);
        let wus = collect_work_units(&store, &types);
        assert_eq!(
            wus,
            BTreeSet::from([None, Some("wu-a".into()), Some("wu-b".into())])
        );
    }

    #[test]
    fn signal_only_protocol_evaluated_once_unscoped() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp.path().join("s"), vec!["doc"]);
        let activations = ActivationStore::load(tmp.path()).unwrap();
        let signals = HashSet::from(["go".to_string()]);

        let protocol = make_protocol(
            "ground",
            &[],
            &[],
            &["doc"],
            &[],
            TriggerCondition::OnSignal { name: "go".into() },
        );

        let candidates = discover_ready_candidates(
            &[protocol],
            &store,
            &activations,
            &signals,
            &["ground"],
            &HashSet::new(),
        );

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].protocol_name, "ground");
        assert_eq!(candidates[0].work_unit, None);
    }

    #[test]
    fn artifact_trigger_discovers_work_units() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["constraints", "implementation"]);
        store
            .record(
                "constraints",
                "a1",
                Path::new("a1.json"),
                &json!({"title": "A", "work_unit": "wu-a"}),
            )
            .unwrap();
        store
            .record(
                "constraints",
                "b1",
                Path::new("b1.json"),
                &json!({"title": "B", "work_unit": "wu-b"}),
            )
            .unwrap();

        let activations = ActivationStore::load(tmp.path()).unwrap();
        let signals = HashSet::new();

        let protocol = make_protocol(
            "implement",
            &["constraints"],
            &[],
            &["implementation"],
            &[],
            TriggerCondition::OnArtifact {
                name: "constraints".into(),
            },
        );

        let candidates = discover_ready_candidates(
            &[protocol],
            &store,
            &activations,
            &signals,
            &["implement"],
            &HashSet::new(),
        );

        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].protocol_name, "implement");
        assert_eq!(candidates[0].work_unit, Some("wu-a".into()));
        assert_eq!(candidates[1].work_unit, Some("wu-b".into()));
    }

    #[test]
    fn completed_suppression_skips_activated_with_passing_postconditions() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["constraints", "implementation"]);
        store
            .record(
                "constraints",
                "a1",
                Path::new("a1.json"),
                &json!({"title": "A", "work_unit": "wu-a"}),
            )
            .unwrap();
        store
            .record(
                "implementation",
                "a1",
                Path::new("impl-a1.json"),
                &json!({"title": "impl-A", "work_unit": "wu-a"}),
            )
            .unwrap();

        let mut activations = ActivationStore::load(tmp.path()).unwrap();
        activations.record_at("implement", Some("wu-a"), 1000);
        let signals = HashSet::new();

        let protocol = make_protocol(
            "implement",
            &["constraints"],
            &[],
            &["implementation"],
            &[],
            TriggerCondition::OnArtifact {
                name: "constraints".into(),
            },
        );

        let candidates = discover_ready_candidates(
            &[protocol],
            &store,
            &activations,
            &signals,
            &["implement"],
            &HashSet::new(),
        );

        assert!(candidates.is_empty());
    }

    #[test]
    fn activated_but_postconditions_fail_still_candidate() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["constraints", "implementation"]);
        store
            .record(
                "constraints",
                "a1",
                Path::new("a1.json"),
                &json!({"title": "A", "work_unit": "wu-a"}),
            )
            .unwrap();
        // No implementation artifact → postconditions fail → not suppressed.

        let mut activations = ActivationStore::load(tmp.path()).unwrap();
        activations.record_at("implement", Some("wu-a"), 1000);
        let signals = HashSet::new();

        let protocol = make_protocol(
            "implement",
            &["constraints"],
            &[],
            &["implementation"],
            &[],
            TriggerCondition::OnArtifact {
                name: "constraints".into(),
            },
        );

        let candidates = discover_ready_candidates(
            &[protocol],
            &store,
            &activations,
            &signals,
            &["implement"],
            &HashSet::new(),
        );

        assert_eq!(candidates.len(), 1);
    }

    #[test]
    fn on_change_trigger_not_suppressed_even_when_postconditions_pass() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["constraints", "implementation"]);
        // Constraints modified at timestamp 2000 (after activation at 1000).
        store
            .record_with_timestamp(
                "constraints",
                "a1",
                Path::new("a1.json"),
                &json!({"title": "A", "work_unit": "wu-a"}),
                2000,
            )
            .unwrap();
        // Implementation still valid from prior run.
        store
            .record(
                "implementation",
                "a1",
                Path::new("impl-a1.json"),
                &json!({"title": "impl-A", "work_unit": "wu-a"}),
            )
            .unwrap();

        let mut activations = ActivationStore::load(tmp.path()).unwrap();
        activations.record_at("implement", Some("wu-a"), 1000);
        let signals = HashSet::new();

        // on_change trigger: constraints changed at 2000, activation was at 1000.
        let protocol = make_protocol(
            "implement",
            &["constraints"],
            &[],
            &["implementation"],
            &[],
            TriggerCondition::OnChange {
                name: "constraints".into(),
            },
        );

        let candidates = discover_ready_candidates(
            &[protocol],
            &store,
            &activations,
            &signals,
            &["implement"],
            &HashSet::new(),
        );

        // Must NOT be suppressed: on_change was satisfied because input changed.
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].protocol_name, "implement");
    }

    #[test]
    fn on_change_in_all_of_not_suppressed() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["constraints", "implementation"]);
        store
            .record_with_timestamp(
                "constraints",
                "a1",
                Path::new("a1.json"),
                &json!({"title": "A", "work_unit": "wu-a"}),
                2000,
            )
            .unwrap();
        store
            .record(
                "implementation",
                "a1",
                Path::new("impl-a1.json"),
                &json!({"title": "impl-A", "work_unit": "wu-a"}),
            )
            .unwrap();

        let mut activations = ActivationStore::load(tmp.path()).unwrap();
        activations.record_at("implement", Some("wu-a"), 1000);
        let signals = HashSet::from(["go".to_string()]);

        // on_change nested inside all_of: still should not be suppressed.
        let protocol = make_protocol(
            "implement",
            &["constraints"],
            &[],
            &["implementation"],
            &[],
            TriggerCondition::AllOf {
                conditions: vec![
                    TriggerCondition::OnChange {
                        name: "constraints".into(),
                    },
                    TriggerCondition::OnSignal { name: "go".into() },
                ],
            },
        );

        let candidates = discover_ready_candidates(
            &[protocol],
            &store,
            &activations,
            &signals,
            &["implement"],
            &HashSet::new(),
        );

        assert_eq!(candidates.len(), 1);
    }

    #[test]
    fn any_of_with_on_change_suppressed_when_change_not_satisfied() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["constraints", "implementation"]);
        // Constraints recorded BEFORE activation — on_change will not fire.
        store
            .record_with_timestamp(
                "constraints",
                "a1",
                Path::new("a1.json"),
                &json!({"title": "A", "work_unit": "wu-a"}),
                500,
            )
            .unwrap();
        // Implementation still valid from prior run.
        store
            .record(
                "implementation",
                "a1",
                Path::new("impl-a1.json"),
                &json!({"title": "impl-A", "work_unit": "wu-a"}),
            )
            .unwrap();

        let mut activations = ActivationStore::load(tmp.path()).unwrap();
        activations.record_at("implement", Some("wu-a"), 1000);
        let signals = HashSet::from(["go".to_string()]);

        // AnyOf(on_signal("go"), on_change("constraints"))
        // Signal fires, but constraints haven't changed since activation.
        // The trigger is satisfied (via signal), but no on_change was satisfied,
        // so suppression should apply.
        let protocol = make_protocol(
            "implement",
            &["constraints"],
            &[],
            &["implementation"],
            &[],
            TriggerCondition::AnyOf {
                conditions: vec![
                    TriggerCondition::OnSignal { name: "go".into() },
                    TriggerCondition::OnChange {
                        name: "constraints".into(),
                    },
                ],
            },
        );

        let candidates = discover_ready_candidates(
            &[protocol],
            &store,
            &activations,
            &signals,
            &["implement"],
            &HashSet::new(),
        );

        // Must be suppressed: on_change was NOT satisfied, prior outputs valid.
        assert!(candidates.is_empty());
    }

    #[test]
    fn partially_scanned_type_in_requires_skips() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["constraints"]);
        store
            .record(
                "constraints",
                "a1",
                Path::new("a1.json"),
                &json!({"title": "A", "work_unit": "wu-a"}),
            )
            .unwrap();

        let activations = ActivationStore::load(tmp.path()).unwrap();
        let signals = HashSet::new();
        let partial = HashSet::from(["constraints".to_string()]);

        let protocol = make_protocol(
            "implement",
            &["constraints"],
            &[],
            &[],
            &[],
            TriggerCondition::OnArtifact {
                name: "constraints".into(),
            },
        );

        let candidates = discover_ready_candidates(
            &[protocol],
            &store,
            &activations,
            &signals,
            &["implement"],
            &partial,
        );

        assert!(candidates.is_empty());
    }

    #[test]
    fn partially_scanned_trigger_type_not_in_requires_skips() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["lint"]);
        store
            .record(
                "lint",
                "a1",
                Path::new("a1.json"),
                &json!({"title": "A", "work_unit": "wu-a"}),
            )
            .unwrap();

        let activations = ActivationStore::load(tmp.path()).unwrap();
        let signals = HashSet::new();
        let partial = HashSet::from(["lint".to_string()]);

        // lint is only referenced by the trigger, not in requires.
        // Partially scanned trigger type → untrusted data → skip.
        let protocol = make_protocol(
            "fix-lint",
            &[],
            &[],
            &[],
            &[],
            TriggerCondition::OnArtifact {
                name: "lint".into(),
            },
        );

        let candidates = discover_ready_candidates(
            &[protocol],
            &store,
            &activations,
            &signals,
            &["fix-lint"],
            &partial,
        );

        assert!(candidates.is_empty());
    }

    #[test]
    fn unsatisfied_trigger_not_candidate() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp.path().join("s"), vec!["doc"]);
        let activations = ActivationStore::load(tmp.path()).unwrap();
        let signals = HashSet::new();

        let protocol = make_protocol(
            "ground",
            &[],
            &[],
            &["doc"],
            &[],
            TriggerCondition::OnSignal { name: "go".into() },
        );

        let candidates = discover_ready_candidates(
            &[protocol],
            &store,
            &activations,
            &signals,
            &["ground"],
            &HashSet::new(),
        );

        assert!(candidates.is_empty());
    }

    #[test]
    fn preconditions_fail_not_candidate() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp.path().join("s"), vec!["constraints", "implementation"]);
        // constraints is required but missing.
        let activations = ActivationStore::load(tmp.path()).unwrap();
        let signals = HashSet::from(["go".to_string()]);

        let protocol = make_protocol(
            "implement",
            &["constraints"],
            &[],
            &["implementation"],
            &[],
            TriggerCondition::OnSignal { name: "go".into() },
        );

        let candidates = discover_ready_candidates(
            &[protocol],
            &store,
            &activations,
            &signals,
            &["implement"],
            &HashSet::new(),
        );

        assert!(candidates.is_empty());
    }

    #[test]
    fn topological_order_determines_candidate_order() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["a", "b"]);
        store
            .record("a", "x", Path::new("a.json"), &json!({"title": "A"}))
            .unwrap();
        store
            .record("b", "x", Path::new("b.json"), &json!({"title": "B"}))
            .unwrap();

        let activations = ActivationStore::load(tmp.path()).unwrap();
        let signals = HashSet::new();

        let protocols = vec![
            make_protocol(
                "alpha",
                &["a"],
                &[],
                &[],
                &[],
                TriggerCondition::OnArtifact { name: "a".into() },
            ),
            make_protocol(
                "beta",
                &["b"],
                &[],
                &[],
                &[],
                TriggerCondition::OnArtifact { name: "b".into() },
            ),
        ];

        // beta first in topological order.
        let candidates = discover_ready_candidates(
            &protocols,
            &store,
            &activations,
            &signals,
            &["beta", "alpha"],
            &HashSet::new(),
        );

        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].protocol_name, "beta");
        assert_eq!(candidates[1].protocol_name, "alpha");
    }
}
