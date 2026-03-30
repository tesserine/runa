//! Dry-run cascade projection from graph state.
//!
//! Projects the full optimistic execution sequence to quiescence without
//! executing any agents. Used by `runa run --dry-run` to preview the cascade
//! that would result from declared `produces` outputs.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::model::{ProtocolDeclaration, TriggerCondition};
use crate::selection::{
    Candidate, CandidateWorkUnitMode, CandidateWorkUnitSource, FreshnessInputMode,
    collect_candidate_work_units, protocol_freshness_inputs, protocol_relevant_input_types,
    protocol_scan_incomplete_types,
};
use crate::store::{ArtifactStore, ValidationStatus};

/// Whether a projected candidate is evaluated from current artifact state or
/// from assumed-success outputs of an earlier projected step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionClass {
    /// Ready based on artifacts that already exist in the store.
    Current,
    /// Ready only because an earlier projected step is assumed to produce its inputs.
    Projected,
}

/// A `(protocol, work_unit)` pair in the projected cascade, annotated with
/// whether it is currently ready or only projected-ready.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionCandidate {
    pub protocol_name: String,
    pub work_unit: Option<String>,
    pub projection: ProjectionClass,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CandidateKey {
    protocol_name: String,
    work_unit: Option<String>,
}

#[derive(Debug, Clone)]
struct ProjectedChange {
    artifact_type: String,
    work_unit: Option<String>,
}

/// Project the full optimistic execution cascade to quiescence.
///
/// Starting from `initial_ready` candidates, simulates each execution by
/// recording assumed-success `produces` outputs, then re-evaluates readiness
/// until no new candidates emerge. Candidates whose inputs are already satisfied
/// from real store state are tagged [`ProjectionClass::Current`]; those that
/// depend on projected outputs are tagged [`ProjectionClass::Projected`].
/// Optional `may_produce` outputs do not advance the projection.
pub fn project_cascade(
    protocols: &[ProtocolDeclaration],
    store: &ArtifactStore,
    topological_order: &[&str],
    initial_ready: &[Candidate],
    partially_scanned_types: &HashSet<String>,
) -> Vec<ProjectionCandidate> {
    let protocol_map: HashMap<&str, &ProtocolDeclaration> = protocols
        .iter()
        .map(|protocol| (protocol.name.as_str(), protocol))
        .collect();
    let initial_ready: HashSet<_> = initial_ready
        .iter()
        .map(|candidate| candidate_key(&candidate.protocol_name, candidate.work_unit.as_deref()))
        .collect();
    let mut projection = ProjectionState::new(store, partially_scanned_types);
    let mut exhausted = HashSet::new();
    let mut emitted = HashSet::new();
    let mut plan = Vec::new();

    loop {
        let Some(next) =
            discover_ready_candidates_projection(protocols, &projection, topological_order)
                .into_iter()
                .find(|candidate| {
                    !exhausted.contains(&candidate_key(
                        &candidate.protocol_name,
                        candidate.work_unit.as_deref(),
                    ))
                })
        else {
            break;
        };

        let key = candidate_key(&next.protocol_name, next.work_unit.as_deref());
        let first_emission = emitted.insert(key.clone());
        let projection_class = if initial_ready.contains(&key) && first_emission {
            ProjectionClass::Current
        } else {
            ProjectionClass::Projected
        };
        plan.push(ProjectionCandidate {
            protocol_name: next.protocol_name.clone(),
            work_unit: next.work_unit.clone(),
            projection: projection_class,
        });
        exhausted.insert(key);

        let protocol = protocol_map
            .get(next.protocol_name.as_str())
            .expect("projected protocol must exist");
        let changes = projection.record_protocol_outputs(protocol, next.work_unit.as_deref());
        exhausted.retain(|candidate| {
            let protocol = protocol_map
                .get(candidate.protocol_name.as_str())
                .expect("projected protocol must exist");
            !changes_affect_candidate(protocol, candidate.work_unit.as_deref(), &changes)
        });
    }

    plan
}

fn discover_ready_candidates_projection(
    protocols: &[ProtocolDeclaration],
    projection: &ProjectionState<'_>,
    topological_order: &[&str],
) -> Vec<Candidate> {
    let mut ready = Vec::new();

    for &protocol_name in topological_order {
        let Some(protocol) = protocols
            .iter()
            .find(|protocol| protocol.name == protocol_name)
        else {
            continue;
        };

        let work_units = protocol_work_units_projection(protocol, projection);
        for work_unit in work_units {
            if candidate_is_ready(protocol, projection, work_unit.as_deref()) {
                ready.push(Candidate {
                    protocol_name: protocol.name.clone(),
                    work_unit,
                });
            }
        }
    }

    ready
}

fn candidate_is_ready(
    protocol: &ProtocolDeclaration,
    projection: &ProjectionState<'_>,
    work_unit: Option<&str>,
) -> bool {
    if !trigger_is_satisfied(&protocol.trigger, protocol, projection, work_unit) {
        return false;
    }

    if !protocol_scan_incomplete_types(protocol, projection.partially_scanned_types).is_empty() {
        return false;
    }

    if !preconditions_satisfied(protocol, projection, work_unit) {
        return false;
    }

    !protocol_is_current(protocol, projection, work_unit)
}

fn preconditions_satisfied(
    protocol: &ProtocolDeclaration,
    projection: &ProjectionState<'_>,
    work_unit: Option<&str>,
) -> bool {
    protocol
        .requires
        .iter()
        .all(|artifact_type| projection.type_has_any_valid(artifact_type, work_unit))
}

fn protocol_is_current(
    protocol: &ProtocolDeclaration,
    projection: &ProjectionState<'_>,
    work_unit: Option<&str>,
) -> bool {
    if protocol.produces.is_empty() && protocol.may_produce.is_empty() {
        return false;
    }

    if protocol.produces.iter().any(|artifact_type| {
        projection
            .store
            .scan_gap_affects_work_unit(artifact_type, work_unit)
            || (projection
                .partially_scanned_types
                .contains(artifact_type.as_str())
                && !projection.store.has_any_scan_gap_for_type(artifact_type))
    }) {
        return false;
    }

    let Some(output_timestamp) = derived_completion_timestamp(protocol, projection, work_unit)
    else {
        return false;
    };

    let freshness_inputs = protocol_freshness_inputs(protocol);

    freshness_inputs
        .iter()
        .filter_map(|(artifact_type, mode)| match mode {
            FreshnessInputMode::AnyRecorded => {
                projection.latest_modification_ms(artifact_type, work_unit)
            }
            FreshnessInputMode::ValidOnly => {
                projection.latest_valid_modification_ms(artifact_type, work_unit)
            }
        })
        .max()
        .is_none_or(|latest_input| latest_input <= output_timestamp)
}

fn derived_completion_timestamp(
    protocol: &ProtocolDeclaration,
    projection: &ProjectionState<'_>,
    work_unit: Option<&str>,
) -> Option<u64> {
    if protocol.produces.is_empty() {
        return None;
    }

    if protocol.produces.iter().any(|artifact_type| {
        projection
            .store
            .scan_gap_affects_work_unit(artifact_type, work_unit)
            || (projection
                .partially_scanned_types
                .contains(artifact_type.as_str())
                && !projection.store.has_any_scan_gap_for_type(artifact_type))
            || !projection.type_is_fully_valid(artifact_type, work_unit)
    }) {
        return None;
    }

    protocol
        .produces
        .iter()
        .filter_map(|artifact_type| projection.latest_modification_ms(artifact_type, work_unit))
        .min()
}

fn protocol_work_units_projection(
    protocol: &ProtocolDeclaration,
    projection: &ProjectionState<'_>,
) -> BTreeSet<Option<String>> {
    collect_candidate_work_units(protocol, projection)
}

fn trigger_is_satisfied(
    condition: &TriggerCondition,
    protocol: &ProtocolDeclaration,
    projection: &ProjectionState<'_>,
    work_unit: Option<&str>,
) -> bool {
    match condition {
        TriggerCondition::OnArtifact { name } => projection.type_has_any_valid(name, work_unit),
        TriggerCondition::OnChange { name } => match projection
            .latest_modification_ms(name, work_unit)
        {
            None => false,
            Some(latest) => match derived_completion_timestamp(protocol, projection, work_unit) {
                None => true,
                Some(last_output) => latest > last_output,
            },
        },
        TriggerCondition::OnInvalid { name } => projection.store.has_any_invalid(name, work_unit),
        TriggerCondition::AllOf { conditions } => conditions
            .iter()
            .all(|child| trigger_is_satisfied(child, protocol, projection, work_unit)),
        TriggerCondition::AnyOf { conditions } => {
            !conditions.is_empty()
                && conditions
                    .iter()
                    .any(|child| trigger_is_satisfied(child, protocol, projection, work_unit))
        }
    }
}

fn changes_affect_candidate(
    protocol: &ProtocolDeclaration,
    work_unit: Option<&str>,
    changes: &[ProjectedChange],
) -> bool {
    let relevant_types = protocol_relevant_input_types(protocol);
    changes.iter().any(|change| {
        relevant_types.contains(change.artifact_type.as_str())
            && match work_unit {
                None => true,
                Some(work_unit) => change
                    .work_unit
                    .as_deref()
                    .is_none_or(|change_wu| change_wu == work_unit),
            }
    })
}

fn candidate_key(protocol_name: &str, work_unit: Option<&str>) -> CandidateKey {
    CandidateKey {
        protocol_name: protocol_name.to_string(),
        work_unit: work_unit.map(str::to_owned),
    }
}

struct ProjectionState<'a> {
    store: &'a ArtifactStore,
    partially_scanned_types: &'a HashSet<String>,
    projected_outputs: HashMap<(String, Option<String>), u64>,
    next_timestamp_ms: u64,
}

impl<'a> ProjectionState<'a> {
    fn new(store: &'a ArtifactStore, partially_scanned_types: &'a HashSet<String>) -> Self {
        let next_timestamp_ms = store
            .artifact_type_names()
            .into_iter()
            .filter_map(|artifact_type| store.latest_modification_ms(artifact_type, None))
            .max()
            .unwrap_or(0)
            + 1;

        Self {
            store,
            partially_scanned_types,
            projected_outputs: HashMap::new(),
            next_timestamp_ms,
        }
    }

    fn record_protocol_outputs(
        &mut self,
        protocol: &ProtocolDeclaration,
        work_unit: Option<&str>,
    ) -> Vec<ProjectedChange> {
        let mut changes = Vec::new();
        for artifact_type in &protocol.produces {
            let scoped_work_unit = work_unit.map(str::to_owned);
            self.projected_outputs.insert(
                (artifact_type.clone(), scoped_work_unit.clone()),
                self.next_timestamp_ms,
            );
            self.next_timestamp_ms += 1;
            changes.push(ProjectedChange {
                artifact_type: artifact_type.clone(),
                work_unit: scoped_work_unit,
            });
        }
        changes
    }

    fn projected_work_units(&self, artifact_type: &str) -> BTreeSet<Option<String>> {
        self.projected_outputs
            .keys()
            .filter(|(projected_type, _)| projected_type == artifact_type)
            .map(|(_, work_unit)| work_unit.clone())
            .collect()
    }

    fn latest_modification_ms(&self, artifact_type: &str, work_unit: Option<&str>) -> Option<u64> {
        self.store
            .latest_modification_ms(artifact_type, work_unit)
            .into_iter()
            .chain(
                self.projected_outputs
                    .iter()
                    .filter(|((projected_type, projected_work_unit), _)| {
                        projected_type == artifact_type
                            && matches_projected_work_unit(
                                projected_work_unit.as_deref(),
                                work_unit,
                            )
                    })
                    .map(|(_, timestamp)| *timestamp),
            )
            .max()
    }

    fn latest_valid_modification_ms(
        &self,
        artifact_type: &str,
        work_unit: Option<&str>,
    ) -> Option<u64> {
        self.store
            .latest_valid_modification_ms(artifact_type, work_unit)
            .into_iter()
            .chain(
                self.projected_outputs
                    .iter()
                    .filter(|((projected_type, projected_work_unit), _)| {
                        projected_type == artifact_type
                            && matches_projected_work_unit(
                                projected_work_unit.as_deref(),
                                work_unit,
                            )
                    })
                    .map(|(_, timestamp)| *timestamp),
            )
            .max()
    }

    fn type_has_any_valid(&self, artifact_type: &str, work_unit: Option<&str>) -> bool {
        self.store.has_any_valid(artifact_type, work_unit)
            || self
                .projected_outputs
                .iter()
                .any(|((projected_type, projected_work_unit), _)| {
                    projected_type == artifact_type
                        && matches_projected_work_unit(projected_work_unit.as_deref(), work_unit)
                })
    }

    fn type_is_fully_valid(&self, artifact_type: &str, work_unit: Option<&str>) -> bool {
        let real_instances = self.store.instances_of(artifact_type, work_unit);
        let has_real_invalid = real_instances
            .iter()
            .any(|(_, state)| !matches!(state.status, ValidationStatus::Valid));
        if has_real_invalid {
            return false;
        }

        real_instances
            .iter()
            .any(|(_, state)| matches!(state.status, ValidationStatus::Valid))
            || self
                .projected_outputs
                .iter()
                .any(|((projected_type, projected_work_unit), _)| {
                    projected_type == artifact_type
                        && matches_projected_work_unit(projected_work_unit.as_deref(), work_unit)
                })
    }
}

impl CandidateWorkUnitSource for ProjectionState<'_> {
    fn is_partially_scanned(&self, artifact_type: &str) -> bool {
        self.partially_scanned_types.contains(artifact_type)
    }

    fn artifact_work_units(
        &self,
        artifact_type: &str,
        mode: CandidateWorkUnitMode,
    ) -> BTreeSet<Option<String>> {
        let mut work_units: BTreeSet<_> = self
            .store
            .instances_of(artifact_type, None)
            .into_iter()
            .filter_map(|(_, state)| match mode {
                CandidateWorkUnitMode::Valid if matches!(state.status, ValidationStatus::Valid) => {
                    Some(state.work_unit.clone())
                }
                CandidateWorkUnitMode::Invalid
                    if matches!(
                        state.status,
                        ValidationStatus::Invalid(_) | ValidationStatus::Malformed(_)
                    ) =>
                {
                    Some(state.work_unit.clone())
                }
                CandidateWorkUnitMode::Recorded => Some(state.work_unit.clone()),
                _ => None,
            })
            .collect();

        if matches!(
            mode,
            CandidateWorkUnitMode::Valid | CandidateWorkUnitMode::Recorded
        ) {
            work_units.extend(self.projected_work_units(artifact_type));
        }

        work_units
    }
}

fn matches_projected_work_unit(
    projected_work_unit: Option<&str>,
    candidate_work_unit: Option<&str>,
) -> bool {
    match candidate_work_unit {
        None => true,
        Some(candidate_work_unit) => projected_work_unit
            .is_none_or(|projected_work_unit| projected_work_unit == candidate_work_unit),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::make_store;
    use serde_json::json;
    use std::path::Path;
    use tempfile::TempDir;

    fn protocol(
        name: &str,
        requires: &[&str],
        produces: &[&str],
        trigger: TriggerCondition,
    ) -> ProtocolDeclaration {
        ProtocolDeclaration {
            name: name.into(),
            requires: requires.iter().map(|value| value.to_string()).collect(),
            accepts: Vec::new(),
            produces: produces.iter().map(|value| value.to_string()).collect(),
            may_produce: Vec::new(),
            trigger,
            instructions: None,
        }
    }

    #[test]
    fn projection_uses_projected_work_units_for_non_object_outputs() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(
            &tmp.path().join("store"),
            vec!["input", "constrained", "done"],
        );
        store
            .record(
                "input",
                "a",
                Path::new("a.json"),
                &json!({"title":"a","work_unit":"wu-a"}),
            )
            .unwrap();
        store
            .record(
                "input",
                "b",
                Path::new("b.json"),
                &json!({"title":"b","work_unit":"wu-b"}),
            )
            .unwrap();

        let protocols = vec![
            protocol(
                "build",
                &["input"],
                &["constrained"],
                TriggerCondition::OnArtifact {
                    name: "input".into(),
                },
            ),
            protocol(
                "verify",
                &["constrained"],
                &["done"],
                TriggerCondition::OnArtifact {
                    name: "constrained".into(),
                },
            ),
        ];

        let plan = project_cascade(
            &protocols,
            &store,
            &["build", "verify"],
            &[
                Candidate {
                    protocol_name: "build".into(),
                    work_unit: Some("wu-a".into()),
                },
                Candidate {
                    protocol_name: "build".into(),
                    work_unit: Some("wu-b".into()),
                },
            ],
            &HashSet::new(),
        );

        assert_eq!(plan.len(), 4);
        assert_eq!(plan[0].work_unit.as_deref(), Some("wu-a"));
        assert_eq!(plan[1].work_unit.as_deref(), Some("wu-b"));
        assert_eq!(plan[2].work_unit.as_deref(), Some("wu-a"));
        assert_eq!(plan[3].work_unit.as_deref(), Some("wu-b"));
        assert_eq!(plan[2].projection, ProjectionClass::Projected);
        assert_eq!(plan[3].projection, ProjectionClass::Projected);
    }

    #[test]
    fn invalid_sibling_reopens_on_artifact_projection() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("store"), vec!["request", "published"]);
        store
            .record_with_timestamp(
                "request",
                "good",
                Path::new("good.json"),
                &json!({"title":"good","work_unit":"wu-a"}),
                1000,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "published",
                "good",
                Path::new("published.json"),
                &json!({"title":"published","work_unit":"wu-a"}),
                2000,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "request",
                "bad",
                Path::new("bad.json"),
                &json!({"work_unit":"wu-a"}),
                3000,
            )
            .unwrap();

        let protocol = protocol(
            "publish",
            &["request"],
            &["published"],
            TriggerCondition::OnArtifact {
                name: "request".into(),
            },
        );
        let partials = HashSet::new();
        let projection = ProjectionState::new(&store, &partials);

        let ready = discover_ready_candidates_projection(&[protocol], &projection, &["publish"]);
        assert_eq!(
            ready,
            vec![Candidate {
                protocol_name: "publish".into(),
                work_unit: Some("wu-a".into()),
            }]
        );
    }

    #[test]
    fn unscoped_valid_artifact_does_not_project_invalid_scoped_sibling() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("store"), vec!["request", "published"]);
        store
            .record(
                "request",
                "shared",
                Path::new("shared.json"),
                &json!({"title":"shared"}),
            )
            .unwrap();
        store
            .record(
                "request",
                "bad",
                Path::new("bad.json"),
                &json!({"work_unit":"wu-a"}),
            )
            .unwrap();

        let protocol = protocol(
            "publish",
            &["request"],
            &["published"],
            TriggerCondition::OnArtifact {
                name: "request".into(),
            },
        );
        let partials = HashSet::new();
        let projection = ProjectionState::new(&store, &partials);

        let ready = discover_ready_candidates_projection(&[protocol], &projection, &["publish"]);
        assert_eq!(
            ready,
            vec![Candidate {
                protocol_name: "publish".into(),
                work_unit: None,
            }]
        );
    }

    #[test]
    fn previously_valid_sibling_becoming_invalid_reopens_on_artifact_projection() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("store"), vec!["request", "published"]);
        store
            .record_with_timestamp(
                "request",
                "a",
                Path::new("a.json"),
                &json!({"title":"a","work_unit":"wu-a"}),
                1000,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "request",
                "b",
                Path::new("b.json"),
                &json!({"title":"b","work_unit":"wu-a"}),
                1500,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "published",
                "good",
                Path::new("published.json"),
                &json!({"title":"published","work_unit":"wu-a"}),
                2000,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "request",
                "b",
                Path::new("b.json"),
                &json!({"work_unit":"wu-a"}),
                3000,
            )
            .unwrap();

        let protocol = protocol(
            "publish",
            &["request"],
            &["published"],
            TriggerCondition::OnArtifact {
                name: "request".into(),
            },
        );
        let partials = HashSet::new();
        let projection = ProjectionState::new(&store, &partials);

        let ready = discover_ready_candidates_projection(&[protocol], &projection, &["publish"]);
        assert_eq!(
            ready,
            vec![Candidate {
                protocol_name: "publish".into(),
                work_unit: Some("wu-a".into()),
            }]
        );
    }
}
