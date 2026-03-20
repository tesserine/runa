use std::collections::{BTreeSet, HashSet};

use crate::model::{ProtocolDeclaration, TriggerCondition};
use crate::store::ArtifactStore;
use crate::trigger::{
    TriggerContext, TriggerResult, derived_completion_timestamp, evaluate as evaluate_trigger,
};

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

        // Scan trust: skip protocol entirely if any requires or trigger-
        // referenced type is partially scanned. Evaluating triggers or
        // preconditions against incomplete data could lead to false activation.
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

        // Collect distinct work_unit values from referenced artifact instances.
        let work_units = collect_work_units(store, &referenced_types, partially_scanned_types);

        for wu in &work_units {
            let wu_ref = wu.as_deref();

            // Build trigger context scoped to this work_unit.
            let ctx = TriggerContext {
                store,
                active_signals,
                work_unit: wu_ref,
            };

            // Evaluate trigger.
            if !matches!(
                evaluate_trigger(&protocol.trigger, protocol, &ctx),
                TriggerResult::Satisfied
            ) {
                continue;
            }

            // Check preconditions.
            if crate::enforce_preconditions(protocol, store, wu_ref).is_err() {
                continue;
            }

            if protocol_is_current(
                protocol,
                store,
                &referenced_types,
                wu_ref,
                partially_scanned_types,
            ) {
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

fn protocol_is_current(
    protocol: &ProtocolDeclaration,
    store: &ArtifactStore,
    referenced_types: &HashSet<&str>,
    work_unit: Option<&str>,
    partially_scanned_types: &HashSet<String>,
) -> bool {
    if protocol.produces.is_empty() && protocol.may_produce.is_empty() {
        return false;
    }

    if protocol
        .produces
        .iter()
        .chain(protocol.may_produce.iter())
        .any(|artifact_type| partially_scanned_types.contains(artifact_type.as_str()))
    {
        return false;
    }

    let Some(output_timestamp) = derived_completion_timestamp(protocol, store, work_unit) else {
        return false;
    };

    referenced_types
        .iter()
        .filter_map(|artifact_type| store.latest_modification_ms(artifact_type, work_unit))
        .max()
        .is_none_or(|latest_input| latest_input <= output_timestamp)
}

/// Collect distinct work_unit values from artifact instances across multiple types.
///
/// Returns `BTreeSet` for deterministic lexicographic ordering. If no instances
/// reference any work_unit, returns `{None}` so the protocol is evaluated once
/// unscoped.
fn collect_work_units(
    store: &ArtifactStore,
    artifact_types: &HashSet<&str>,
    partially_scanned_types: &HashSet<String>,
) -> BTreeSet<Option<String>> {
    let mut work_units = BTreeSet::new();

    for &type_name in artifact_types {
        if partially_scanned_types.contains(type_name) {
            continue;
        }
        for (_, state) in store.instances_of(type_name, None) {
            work_units.insert(state.work_unit.clone());
        }
    }

    // Drop the unscoped entry when scoped work units are present.
    // Scoped queries already include unscoped instances (via
    // matches_work_unit_filter), so the None entry would create a
    // redundant candidate that duplicates per-work-unit runs.
    //
    // None candidates appear naturally for planning-phase protocols
    // (survey, decompose) and the phase bridge (begin), whose
    // inputs predate work-unit identity. Execution-phase protocols
    // always have scoped inputs, so None is always removed for them.
    if work_units.iter().any(|wu| wu.is_some()) {
        work_units.remove(&None);
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

    #[derive(Default)]
    struct CompletionStore;

    impl CompletionStore {
        fn load(_path: &Path) -> Result<Self, std::io::Error> {
            Ok(Self)
        }

        fn record_at(&mut self, _protocol: &str, _work_unit: Option<&str>, _timestamp_ms: u64) {}
    }

    fn discover_ready_candidates(
        protocols: &[ProtocolDeclaration],
        store: &ArtifactStore,
        _completions: &CompletionStore,
        active_signals: &HashSet<String>,
        topological_order: &[&str],
        partially_scanned_types: &HashSet<String>,
    ) -> Vec<Candidate> {
        super::discover_ready_candidates(
            protocols,
            store,
            active_signals,
            topological_order,
            partially_scanned_types,
        )
    }

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
        let wus = collect_work_units(&store, &types, &HashSet::new());
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
        let wus = collect_work_units(&store, &types, &HashSet::new());
        assert_eq!(
            wus,
            BTreeSet::from([Some("wu-a".into()), Some("wu-b".into())])
        );
    }

    #[test]
    fn collect_work_units_keeps_none_when_all_unscoped() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc"]);
        store
            .record("doc", "a1", Path::new("a1.json"), &json!({"title": "A"}))
            .unwrap();
        store
            .record("doc", "b1", Path::new("b1.json"), &json!({"title": "B"}))
            .unwrap();

        let types = HashSet::from(["doc"]);
        let wus = collect_work_units(&store, &types, &HashSet::new());
        assert_eq!(wus, BTreeSet::from([None]));
    }

    #[test]
    fn collect_work_units_excludes_partially_scanned_types() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["notes", "constraints"]);
        store
            .record(
                "notes",
                "wu-stale",
                Path::new("notes-stale.json"),
                &json!({"title": "Stale", "work_unit": "wu-stale"}),
            )
            .unwrap();
        store
            .record(
                "constraints",
                "wu-good",
                Path::new("constraints-good.json"),
                &json!({"title": "Good", "work_unit": "wu-good"}),
            )
            .unwrap();

        let types = HashSet::from(["notes", "constraints"]);
        let partial = HashSet::from(["notes".to_string()]);
        let wus = collect_work_units(&store, &types, &partial);
        assert_eq!(wus, BTreeSet::from([Some("wu-good".into())]));
    }

    #[test]
    fn signal_only_protocol_evaluated_once_unscoped() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp.path().join("s"), vec!["doc"]);
        let completions = CompletionStore::load(tmp.path()).unwrap();
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
            &completions,
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

        let completions = CompletionStore::load(tmp.path()).unwrap();
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
            &completions,
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

        let mut completions = CompletionStore::load(tmp.path()).unwrap();
        completions.record_at("implement", Some("wu-a"), 1000);
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
            &completions,
            &signals,
            &["implement"],
            &HashSet::new(),
        );

        assert!(candidates.is_empty());
    }

    #[test]
    fn stale_outputs_do_not_suppress_candidates() {
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
            .record_with_timestamp(
                "implementation",
                "a1",
                Path::new("impl-a1.json"),
                &json!({"title": "impl-A", "work_unit": "wu-a"}),
                1000,
            )
            .unwrap();

        let mut completions = CompletionStore::load(tmp.path()).unwrap();
        completions.record_at("implement", Some("wu-a"), 3000);
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
            &completions,
            &signals,
            &["implement"],
            &HashSet::new(),
        );

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].protocol_name, "implement");
        assert_eq!(candidates[0].work_unit, Some("wu-a".into()));
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

        let mut completions = CompletionStore::load(tmp.path()).unwrap();
        completions.record_at("implement", Some("wu-a"), 1000);
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
            &completions,
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
        // Constraints modified at timestamp 2000 (after completion at 1000).
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
            .record_with_timestamp(
                "implementation",
                "a1",
                Path::new("impl-a1.json"),
                &json!({"title": "impl-A", "work_unit": "wu-a"}),
                1000,
            )
            .unwrap();

        let mut completions = CompletionStore::load(tmp.path()).unwrap();
        completions.record_at("implement", Some("wu-a"), 1000);
        let signals = HashSet::new();

        // on_change trigger: constraints changed at 2000, completion was at 1000.
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
            &completions,
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
            .record_with_timestamp(
                "implementation",
                "a1",
                Path::new("impl-a1.json"),
                &json!({"title": "impl-A", "work_unit": "wu-a"}),
                1000,
            )
            .unwrap();

        let mut completions = CompletionStore::load(tmp.path()).unwrap();
        completions.record_at("implement", Some("wu-a"), 1000);
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
            &completions,
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
        // Constraints recorded BEFORE completion — on_change will not fire.
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

        let mut completions = CompletionStore::load(tmp.path()).unwrap();
        completions.record_at("implement", Some("wu-a"), 1000);
        let signals = HashSet::from(["go".to_string()]);

        // AnyOf(on_signal("go"), on_change("constraints"))
        // Signal fires, but constraints haven't changed since completion.
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
            &completions,
            &signals,
            &["implement"],
            &HashSet::new(),
        );

        // Must be suppressed: on_change was NOT satisfied, prior outputs valid.
        assert!(candidates.is_empty());
    }

    #[test]
    fn any_of_on_change_in_unsatisfied_branch_suppressed() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["constraints", "implementation"]);
        // Constraints modified at timestamp 2000 (after completion at 1000).
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

        let mut completions = CompletionStore::load(tmp.path()).unwrap();
        completions.record_at("implement", Some("wu-a"), 1000);
        // Signal "y" is active, signal "x" is not.
        let signals = HashSet::from(["y".to_string()]);

        // any_of(all_of(on_change(constraints), on_signal("x")), on_signal("y"))
        // The all_of branch is NOT satisfied (x inactive), so on_change in it
        // should not count. The trigger fires via on_signal("y") only.
        let protocol = make_protocol(
            "implement",
            &["constraints"],
            &[],
            &["implementation"],
            &[],
            TriggerCondition::AnyOf {
                conditions: vec![
                    TriggerCondition::AllOf {
                        conditions: vec![
                            TriggerCondition::OnChange {
                                name: "constraints".into(),
                            },
                            TriggerCondition::OnSignal { name: "x".into() },
                        ],
                    },
                    TriggerCondition::OnSignal { name: "y".into() },
                ],
            },
        );

        let candidates = discover_ready_candidates(
            &[protocol],
            &store,
            &completions,
            &signals,
            &["implement"],
            &HashSet::new(),
        );

        // Must be suppressed: the on_change is in an unsatisfied branch,
        // the trigger fires via on_signal("y"), and postconditions pass.
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

        let completions = CompletionStore::load(tmp.path()).unwrap();
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
            &completions,
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

        let completions = CompletionStore::load(tmp.path()).unwrap();
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
            &completions,
            &signals,
            &["fix-lint"],
            &partial,
        );

        assert!(candidates.is_empty());
    }

    #[test]
    fn partially_scanned_output_type_skips_completed_suppression() {
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
        // Output artifact present — postconditions would pass on full scan.
        store
            .record(
                "implementation",
                "a1",
                Path::new("impl-a1.json"),
                &json!({"title": "impl-A", "work_unit": "wu-a"}),
            )
            .unwrap();

        let mut completions = CompletionStore::load(tmp.path()).unwrap();
        completions.record_at("implement", Some("wu-a"), 1000);
        let signals = HashSet::new();
        // implementation is partially scanned — postcondition data is stale.
        let partial = HashSet::from(["implementation".to_string()]);

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
            &completions,
            &signals,
            &["implement"],
            &partial,
        );

        // Must NOT be suppressed: output type was partially scanned,
        // so postcondition check against stale data is untrusted.
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].protocol_name, "implement");
        assert_eq!(candidates[0].work_unit, Some("wu-a".into()));
    }

    #[test]
    fn unsatisfied_trigger_not_candidate() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp.path().join("s"), vec!["doc"]);
        let completions = CompletionStore::load(tmp.path()).unwrap();
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
            &completions,
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
        let completions = CompletionStore::load(tmp.path()).unwrap();
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
            &completions,
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

        let completions = CompletionStore::load(tmp.path()).unwrap();
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
            &completions,
            &signals,
            &["beta", "alpha"],
            &HashSet::new(),
        );

        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].protocol_name, "beta");
        assert_eq!(candidates[1].protocol_name, "alpha");
    }
}
