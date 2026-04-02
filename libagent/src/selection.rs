//! Scope-driven protocol readiness evaluation and candidate classification.
//!
//! Evaluates protocols in topological order to discover which `(protocol, work_unit)`
//! pairs are ready for execution, blocked on preconditions, or waiting on triggers.
//! The main entry points are [`discover_ready_candidates`] (returns only ready pairs)
//! and [`classify_candidates`] (returns all pairs with their classification).

use std::collections::{HashMap, HashSet};

use crate::enforcement::ArtifactFailure;
use crate::graph::{CycleError, DependencyGraph};
use crate::model::{ProtocolDeclaration, TriggerCondition};
use crate::store::ArtifactStore;
use crate::trigger::{
    TriggerContext, TriggerResult, derived_completion_timestamp, evaluate as evaluate_trigger,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvaluationScope<'a> {
    Unscoped,
    Scoped(&'a str),
}

#[derive(Debug, Clone, PartialEq)]
pub struct EvaluationTopology {
    pub status_order: Vec<String>,
    pub execution_order: Vec<String>,
    pub cycle: Option<CycleError>,
}

/// A (protocol, work_unit) pair that is ready for execution.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub protocol_name: String,
    pub work_unit: Option<String>,
}

/// Classification status for a (protocol, work_unit) candidate.
#[derive(Debug, Clone, PartialEq)]
pub enum CandidateStatus {
    /// Trigger satisfied, preconditions pass, outputs not current.
    Ready,
    /// Trigger satisfied but preconditions fail, or scan incomplete.
    Blocked {
        precondition_failures: Vec<ArtifactFailure>,
        scan_incomplete_types: Vec<String>,
    },
    /// Trigger not satisfied, or outputs are already current.
    Waiting { unsatisfied_conditions: Vec<String> },
}

/// A (protocol, work_unit) pair with its classification and scan trust.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassifiedCandidate {
    pub protocol_name: String,
    pub work_unit: Option<String>,
    pub status: CandidateStatus,
    pub trigger_satisfied: bool,
    pub scan_trust: ScanTrust,
}

/// Scan trust information for a classified candidate.
#[derive(Debug, Clone, PartialEq)]
pub struct ScanTrust {
    pub trusted: bool,
    pub incomplete_types: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FreshnessInputMode {
    AnyRecorded,
    #[allow(dead_code)] // Reserved for future input-set tracking.
    ValidOnly,
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
    topological_order: &[&str],
    partially_scanned_types: &HashSet<String>,
    scope: EvaluationScope<'_>,
) -> Vec<Candidate> {
    classify_candidates(
        protocols,
        store,
        topological_order,
        partially_scanned_types,
        scope,
    )
    .into_iter()
    .filter(|c| matches!(c.status, CandidateStatus::Ready))
    .map(|c| Candidate {
        protocol_name: c.protocol_name,
        work_unit: c.work_unit,
    })
    .collect()
}

pub fn resolve_evaluation_topology(
    protocols: &[ProtocolDeclaration],
    graph: &DependencyGraph,
    scope: EvaluationScope<'_>,
) -> EvaluationTopology {
    let excluded: HashSet<&str> = protocols
        .iter()
        .filter(|protocol| !protocol_in_scope(protocol, scope))
        .map(|protocol| protocol.name.as_str())
        .collect();

    match graph.topological_order_filtered(&excluded) {
        Ok(order) => EvaluationTopology {
            status_order: order.iter().map(|name| (*name).to_string()).collect(),
            execution_order: order.iter().map(|name| (*name).to_string()).collect(),
            cycle: None,
        },
        Err(cycle) => {
            let cycle_participants: HashSet<&str> =
                cycle.path.iter().map(|name| name.as_str()).collect();
            let combined_exclusions: HashSet<&str> = excluded
                .iter()
                .copied()
                .chain(cycle_participants.iter().copied())
                .collect();
            let execution_order: Vec<String> = graph
                .topological_order_excluding(&combined_exclusions)
                .into_iter()
                .map(str::to_owned)
                .collect();
            let mut status_order = execution_order.clone();
            status_order.extend(
                protocols
                    .iter()
                    .filter(|protocol| protocol_in_scope(protocol, scope))
                    .map(|protocol| protocol.name.as_str())
                    .filter(|name| cycle_participants.contains(name))
                    .map(str::to_owned),
            );

            EvaluationTopology {
                status_order,
                execution_order,
                cycle: Some(cycle),
            }
        }
    }
}

/// Returns the `requires` and trigger-referenced artifact types for this protocol
/// that have incomplete scan results, in a stable order (requires first, then
/// trigger types lexicographically).
pub fn protocol_scan_incomplete_types(
    protocol: &ProtocolDeclaration,
    partially_scanned_types: &HashSet<String>,
) -> Vec<String> {
    let mut trigger_types = HashSet::new();
    trigger_artifact_types(&protocol.trigger, &mut trigger_types);

    let mut incomplete = Vec::new();
    for artifact_type in &protocol.requires {
        if partially_scanned_types.contains(artifact_type.as_str())
            && !incomplete.contains(artifact_type)
        {
            incomplete.push(artifact_type.clone());
        }
    }

    let mut trigger_type_names: Vec<&str> = trigger_types.into_iter().collect();
    trigger_type_names.sort_unstable();
    for artifact_type in trigger_type_names {
        if partially_scanned_types.contains(artifact_type)
            && !incomplete.iter().any(|existing| existing == artifact_type)
        {
            incomplete.push(artifact_type.to_string());
        }
    }

    incomplete
}

/// Collect the artifact type names that affect freshness for this protocol:
/// `requires` types plus any types referenced in the trigger condition tree.
pub fn protocol_relevant_input_types(protocol: &ProtocolDeclaration) -> HashSet<String> {
    protocol_freshness_inputs(protocol).into_keys().collect()
}

/// Check whether a scan result contains changes to artifact types that affect
/// this protocol's freshness evaluation for the given work unit.
pub fn protocol_relevant_inputs_changed(
    protocol: &ProtocolDeclaration,
    work_unit: Option<&str>,
    scan_result: &crate::ScanResult,
) -> bool {
    let relevant_types = protocol_relevant_input_types(protocol);

    scan_result.new.iter().any(|artifact| {
        scan_change_affects_candidate(
            artifact.artifact_type.as_str(),
            artifact.work_unit.as_deref(),
            work_unit,
            &relevant_types,
        )
    }) || scan_result.modified.iter().any(|artifact| {
        scan_change_affects_candidate(
            artifact.artifact_type.as_str(),
            artifact.work_unit.as_deref(),
            work_unit,
            &relevant_types,
        )
    }) || scan_result.revalidated.iter().any(|artifact| {
        scan_change_affects_candidate(
            artifact.artifact_type.as_str(),
            artifact.work_unit.as_deref(),
            work_unit,
            &relevant_types,
        )
    }) || scan_result.removed.iter().any(|artifact| {
        scan_change_affects_candidate(
            artifact.artifact_type.as_str(),
            artifact.work_unit.as_deref(),
            work_unit,
            &relevant_types,
        )
    }) || scan_result.invalid.iter().any(|artifact| {
        scan_change_affects_candidate(
            artifact.artifact_type.as_str(),
            artifact.work_unit.as_deref(),
            work_unit,
            &relevant_types,
        )
    }) || scan_result.malformed.iter().any(|artifact| {
        scan_change_affects_candidate(
            artifact.artifact_type.as_str(),
            artifact.work_unit.as_deref(),
            work_unit,
            &relevant_types,
        )
    })
}

fn scan_change_affects_candidate(
    artifact_type: &str,
    change_work_unit: Option<&str>,
    candidate_work_unit: Option<&str>,
    relevant_types: &HashSet<String>,
) -> bool {
    relevant_types.contains(artifact_type)
        && match candidate_work_unit {
            None => true,
            Some(candidate_work_unit) => change_work_unit
                .is_none_or(|change_work_unit| change_work_unit == candidate_work_unit),
        }
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
    freshness_inputs: &HashMap<String, FreshnessInputMode>,
    work_unit: Option<&str>,
    partially_scanned_types: &HashSet<String>,
) -> bool {
    if protocol.produces.is_empty() && protocol.may_produce.is_empty() {
        return false;
    }

    if protocol.produces.iter().any(|artifact_type| {
        store.scan_gap_affects_work_unit(artifact_type, work_unit)
            || (partially_scanned_types.contains(artifact_type.as_str())
                && !store.has_any_scan_gap_for_type(artifact_type))
    }) {
        return false;
    }

    if protocol.may_produce.iter().any(|artifact_type| {
        !store.instances_of(artifact_type, work_unit).is_empty()
            && (store.scan_gap_affects_work_unit(artifact_type, work_unit)
                || (partially_scanned_types.contains(artifact_type.as_str())
                    && !store.has_any_scan_gap_for_type(artifact_type)))
    }) {
        return false;
    }

    let Some(output_timestamp) =
        derived_completion_timestamp(protocol, store, work_unit, partially_scanned_types)
    else {
        return false;
    };

    freshness_inputs
        .iter()
        .filter_map(|(artifact_type, mode)| match mode {
            FreshnessInputMode::AnyRecorded => {
                store.latest_modification_ms(artifact_type, work_unit)
            }
            FreshnessInputMode::ValidOnly => {
                store.latest_valid_modification_ms(artifact_type, work_unit)
            }
        })
        .max()
        .is_none_or(|latest_input| latest_input <= output_timestamp)
}

fn record_freshness_input(
    freshness_inputs: &mut HashMap<String, FreshnessInputMode>,
    artifact_type: &str,
    mode: FreshnessInputMode,
) {
    freshness_inputs
        .entry(artifact_type.to_string())
        .and_modify(|existing| {
            if mode == FreshnessInputMode::AnyRecorded {
                *existing = FreshnessInputMode::AnyRecorded;
            }
        })
        .or_insert(mode);
}

fn trigger_freshness_inputs(
    condition: &TriggerCondition,
    out: &mut HashMap<String, FreshnessInputMode>,
) {
    match condition {
        TriggerCondition::OnArtifact { name } => {
            record_freshness_input(out, name.as_str(), FreshnessInputMode::AnyRecorded);
        }
        TriggerCondition::OnChange { name } | TriggerCondition::OnInvalid { name } => {
            record_freshness_input(out, name.as_str(), FreshnessInputMode::AnyRecorded);
        }
        TriggerCondition::AllOf { conditions } | TriggerCondition::AnyOf { conditions } => {
            for child in conditions {
                trigger_freshness_inputs(child, out);
            }
        }
    }
}

pub(crate) fn protocol_freshness_inputs(
    protocol: &ProtocolDeclaration,
) -> HashMap<String, FreshnessInputMode> {
    let mut freshness_inputs = HashMap::new();
    trigger_freshness_inputs(&protocol.trigger, &mut freshness_inputs);
    for name in &protocol.requires {
        record_freshness_input(
            &mut freshness_inputs,
            name.as_str(),
            FreshnessInputMode::AnyRecorded,
        );
    }
    freshness_inputs
}

// --- Trigger trust evaluation (moved from runa-cli/src/commands/protocol_eval.rs) ---

struct TriggerEvaluation {
    satisfied: bool,
    trusted: bool,
    scan_types: Vec<String>,
}

fn evaluate_trigger_trust(
    condition: &TriggerCondition,
    protocol: &ProtocolDeclaration,
    context: &TriggerContext<'_>,
    affected_types: &HashSet<String>,
) -> TriggerEvaluation {
    match condition {
        TriggerCondition::OnArtifact { name } => primitive_trigger_eval(
            condition,
            protocol,
            context,
            affected_types.contains(name.as_str()),
            true,
            true,
            Some(name.clone()),
        ),
        TriggerCondition::OnInvalid { name } => primitive_trigger_eval(
            condition,
            protocol,
            context,
            affected_types.contains(name.as_str()),
            true,
            false,
            Some(name.clone()),
        ),
        TriggerCondition::OnChange { name } => {
            on_change_trigger_eval(condition, protocol, context, name, affected_types)
        }
        TriggerCondition::AllOf { conditions } => {
            let children: Vec<_> = conditions
                .iter()
                .map(|child| evaluate_trigger_trust(child, protocol, context, affected_types))
                .collect();

            if children.iter().all(|child| child.satisfied) {
                let mut scan_types = Vec::new();
                let mut trusted = true;
                for child in &children {
                    if !child.trusted {
                        trusted = false;
                        append_unique(&mut scan_types, child.scan_types.clone());
                    }
                }
                TriggerEvaluation {
                    satisfied: true,
                    trusted,
                    scan_types,
                }
            } else if children
                .iter()
                .any(|child| !child.satisfied && child.trusted)
            {
                TriggerEvaluation {
                    satisfied: false,
                    trusted: true,
                    scan_types: Vec::new(),
                }
            } else {
                let mut scan_types = Vec::new();
                for child in &children {
                    if !child.trusted {
                        append_unique(&mut scan_types, child.scan_types.clone());
                    }
                }
                TriggerEvaluation {
                    satisfied: false,
                    trusted: false,
                    scan_types,
                }
            }
        }
        TriggerCondition::AnyOf { conditions } => {
            if conditions.is_empty() {
                return TriggerEvaluation {
                    satisfied: false,
                    trusted: true,
                    scan_types: Vec::new(),
                };
            }

            let children: Vec<_> = conditions
                .iter()
                .map(|child| evaluate_trigger_trust(child, protocol, context, affected_types))
                .collect();

            if children
                .iter()
                .any(|child| child.satisfied && child.trusted)
            {
                TriggerEvaluation {
                    satisfied: true,
                    trusted: true,
                    scan_types: Vec::new(),
                }
            } else if children.iter().any(|child| child.satisfied) {
                let mut scan_types = Vec::new();
                for child in &children {
                    if child.satisfied && !child.trusted {
                        append_unique(&mut scan_types, child.scan_types.clone());
                    }
                }
                TriggerEvaluation {
                    satisfied: true,
                    trusted: false,
                    scan_types,
                }
            } else if children
                .iter()
                .all(|child| !child.satisfied && child.trusted)
            {
                TriggerEvaluation {
                    satisfied: false,
                    trusted: true,
                    scan_types: Vec::new(),
                }
            } else {
                let mut scan_types = Vec::new();
                for child in &children {
                    if !child.satisfied && !child.trusted {
                        append_unique(&mut scan_types, child.scan_types.clone());
                    }
                }
                TriggerEvaluation {
                    satisfied: false,
                    trusted: false,
                    scan_types,
                }
            }
        }
    }
}

fn primitive_trigger_eval(
    condition: &TriggerCondition,
    protocol: &ProtocolDeclaration,
    context: &TriggerContext<'_>,
    affected: bool,
    untrustworthy_when_not_satisfied: bool,
    untrustworthy_when_satisfied: bool,
    artifact_type: Option<String>,
) -> TriggerEvaluation {
    let satisfied = matches!(
        evaluate_trigger(condition, protocol, context),
        TriggerResult::Satisfied
    );

    let untrusted = if affected {
        if satisfied {
            untrustworthy_when_satisfied
        } else {
            untrustworthy_when_not_satisfied
        }
    } else {
        false
    };

    TriggerEvaluation {
        satisfied,
        trusted: !untrusted,
        scan_types: if untrusted {
            artifact_type.into_iter().collect()
        } else {
            Vec::new()
        },
    }
}

fn on_change_trigger_eval(
    condition: &TriggerCondition,
    protocol: &ProtocolDeclaration,
    context: &TriggerContext<'_>,
    input_type: &str,
    affected_types: &HashSet<String>,
) -> TriggerEvaluation {
    let satisfied = matches!(
        evaluate_trigger(condition, protocol, context),
        TriggerResult::Satisfied
    );

    let mut trusted = true;
    let mut scan_types = Vec::new();

    if affected_types.contains(input_type) && !satisfied {
        trusted = false;
        scan_types.push(input_type.to_string());
    }
    // Partially scanned outputs only make freshness suppression untrustworthy.
    // They must not make the trigger itself untrustworthy or block reruns.

    TriggerEvaluation {
        satisfied,
        trusted,
        scan_types,
    }
}

fn append_unique(target: &mut Vec<String>, values: Vec<String>) {
    for value in values {
        if !target.contains(&value) {
            target.push(value);
        }
    }
}

/// Recursively collect unsatisfied trigger condition reasons.
pub fn collect_unsatisfied_conditions(
    condition: &TriggerCondition,
    protocol: &ProtocolDeclaration,
    context: &TriggerContext<'_>,
) -> Vec<String> {
    match evaluate_trigger(condition, protocol, context) {
        TriggerResult::Satisfied => Vec::new(),
        TriggerResult::NotSatisfied(reason) => match condition {
            TriggerCondition::AllOf { conditions } => conditions
                .iter()
                .flat_map(|child| collect_unsatisfied_conditions(child, protocol, context))
                .collect(),
            TriggerCondition::AnyOf { conditions } => {
                if conditions.is_empty() {
                    vec![format!("{condition}: {reason}")]
                } else {
                    conditions
                        .iter()
                        .flat_map(|child| collect_unsatisfied_conditions(child, protocol, context))
                        .collect()
                }
            }
            _ => vec![format!("{condition}: {reason}")],
        },
    }
}

// --- Classified candidate discovery ---

/// Classify all (protocol, work_unit) pairs as READY, BLOCKED, or WAITING.
///
/// Evaluates protocols in topological order. For each protocol, discovers
/// candidate work_units and classifies each based on trigger satisfaction,
/// scan trust, preconditions, and output freshness.
///
/// Results are emitted in topological protocol order, with work_units
/// in deterministic lexicographic order within each protocol.
pub fn classify_candidates(
    protocols: &[ProtocolDeclaration],
    store: &ArtifactStore,
    topological_order: &[&str],
    partially_scanned_types: &HashSet<String>,
    scope: EvaluationScope<'_>,
) -> Vec<ClassifiedCandidate> {
    let mut classified = Vec::new();

    for &protocol_name in topological_order {
        let Some(protocol) = protocols.iter().find(|p| p.name == protocol_name) else {
            continue;
        };

        let protocol_scan_failures =
            protocol_scan_incomplete_types(protocol, partially_scanned_types);
        let readiness_scan_failures =
            precondition_scan_incomplete_types(protocol, partially_scanned_types);

        let work_units = candidate_work_units_for_scope(protocol, scope);
        let freshness_inputs = protocol_freshness_inputs(protocol);

        for wu in &work_units {
            let wu_ref = wu.as_deref();

            let context = TriggerContext {
                store,
                work_unit: wu_ref,
                partially_scanned_types,
            };
            let trigger_eval = evaluate_trigger_trust(
                &protocol.trigger,
                protocol,
                &context,
                partially_scanned_types,
            );

            let scan_trust = ScanTrust {
                trusted: trigger_eval.trusted,
                incomplete_types: trigger_eval.scan_types.clone(),
            };

            let trigger_scan_failures = trigger_scan_incomplete_failures(
                protocol,
                partially_scanned_types,
                &trigger_eval.scan_types,
            );

            let status = if trigger_eval.satisfied {
                let mut all_scan_failures = protocol_scan_failures.clone();
                append_unique(&mut all_scan_failures, trigger_scan_failures);

                let precondition_failures =
                    match crate::enforce_preconditions(protocol, store, wu_ref) {
                        Ok(()) => Vec::new(),
                        Err(err) => err.failures,
                    };

                if all_scan_failures.is_empty() && precondition_failures.is_empty() {
                    if protocol_is_current(
                        protocol,
                        store,
                        &freshness_inputs,
                        wu_ref,
                        partially_scanned_types,
                    ) {
                        CandidateStatus::Waiting {
                            unsatisfied_conditions: vec!["outputs are current".to_string()],
                        }
                    } else {
                        CandidateStatus::Ready
                    }
                } else {
                    CandidateStatus::Blocked {
                        precondition_failures,
                        scan_incomplete_types: all_scan_failures,
                    }
                }
            } else if trigger_scan_failures.is_empty() && readiness_scan_failures.is_empty() {
                CandidateStatus::Waiting {
                    unsatisfied_conditions: collect_unsatisfied_conditions(
                        &protocol.trigger,
                        protocol,
                        &context,
                    ),
                }
            } else {
                let mut all_scan_failures = readiness_scan_failures.clone();
                append_unique(&mut all_scan_failures, trigger_scan_failures);

                let precondition_failures =
                    match crate::enforce_preconditions(protocol, store, wu_ref) {
                        Ok(()) => Vec::new(),
                        Err(err) => err.failures,
                    };

                CandidateStatus::Blocked {
                    precondition_failures,
                    scan_incomplete_types: all_scan_failures,
                }
            };

            classified.push(ClassifiedCandidate {
                protocol_name: protocol.name.clone(),
                work_unit: wu.clone(),
                status,
                trigger_satisfied: trigger_eval.satisfied,
                scan_trust,
            });
        }
    }

    classified
}

pub(crate) fn candidate_work_units_for_scope(
    protocol: &ProtocolDeclaration,
    scope: EvaluationScope<'_>,
) -> Vec<Option<String>> {
    match scope {
        EvaluationScope::Unscoped if !protocol.scoped => vec![None],
        EvaluationScope::Scoped(work_unit) if protocol.scoped => {
            vec![Some(work_unit.to_string())]
        }
        _ => Vec::new(),
    }
}

fn protocol_in_scope(protocol: &ProtocolDeclaration, scope: EvaluationScope<'_>) -> bool {
    matches!(
        (scope, protocol.scoped),
        (EvaluationScope::Unscoped, false) | (EvaluationScope::Scoped(_), true)
    )
}

/// Collect scan-incomplete types from trigger eval scan_types and requires types.
fn trigger_scan_incomplete_failures(
    protocol: &ProtocolDeclaration,
    partially_scanned_types: &HashSet<String>,
    trigger_scan_types: &[String],
) -> Vec<String> {
    let mut types = trigger_scan_types.to_vec();

    for artifact_type in &protocol.requires {
        if partially_scanned_types.contains(artifact_type.as_str())
            && !types.contains(artifact_type)
        {
            types.push(artifact_type.clone());
        }
    }

    types
}

/// Collect scan-incomplete types from requires and produces.
fn precondition_scan_incomplete_types(
    protocol: &ProtocolDeclaration,
    partially_scanned_types: &HashSet<String>,
) -> Vec<String> {
    let mut types = Vec::new();
    for artifact_type in &protocol.requires {
        if partially_scanned_types.contains(artifact_type.as_str())
            && !types
                .iter()
                .any(|existing: &String| existing == artifact_type)
        {
            types.push(artifact_type.clone());
        }
    }
    types
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::make_store;
    use serde_json::json;
    use std::path::Path;
    use tempfile::TempDir;

    fn discover_ready_candidates(
        protocols: &[ProtocolDeclaration],
        store: &ArtifactStore,
        topological_order: &[&str],
        partially_scanned_types: &HashSet<String>,
    ) -> Vec<Candidate> {
        super::discover_ready_candidates(
            protocols,
            store,
            topological_order,
            partially_scanned_types,
            EvaluationScope::Unscoped,
        )
    }

    fn discover_ready_candidates_scoped(
        protocols: &[ProtocolDeclaration],
        store: &ArtifactStore,
        topological_order: &[&str],
        partially_scanned_types: &HashSet<String>,
        work_unit: &str,
    ) -> Vec<Candidate> {
        super::discover_ready_candidates(
            protocols,
            store,
            topological_order,
            partially_scanned_types,
            EvaluationScope::Scoped(work_unit),
        )
    }

    fn classify_candidates_scoped(
        protocols: &[ProtocolDeclaration],
        store: &ArtifactStore,
        topological_order: &[&str],
        partially_scanned_types: &HashSet<String>,
        work_unit: &str,
    ) -> Vec<ClassifiedCandidate> {
        super::classify_candidates(
            protocols,
            store,
            topological_order,
            partially_scanned_types,
            EvaluationScope::Scoped(work_unit),
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
            scoped: false,
            trigger,
            instructions: None,
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
    fn protocol_relevant_input_types_collects_requires_and_trigger_types() {
        let protocol = make_protocol(
            "implement",
            &["constraints"],
            &["prior-art"],
            &["implementation"],
            &[],
            TriggerCondition::AnyOf {
                conditions: vec![
                    TriggerCondition::OnArtifact {
                        name: "constraints".into(),
                    },
                    TriggerCondition::OnChange {
                        name: "review".into(),
                    },
                ],
            },
        );

        let types = protocol_relevant_input_types(&protocol);
        assert_eq!(types.len(), 2);
        assert!(types.contains("constraints"));
        assert!(types.contains("review"));
        assert!(!types.contains("prior-art"));
    }

    #[test]
    fn protocol_freshness_inputs_keep_change_semantics_when_type_is_also_required() {
        let protocol = make_protocol(
            "repair",
            &["report"],
            &[],
            &["findings"],
            &[],
            TriggerCondition::OnChange {
                name: "report".into(),
            },
        );

        let inputs = protocol_freshness_inputs(&protocol);

        assert_eq!(inputs.get("report"), Some(&FreshnessInputMode::AnyRecorded));
    }

    #[test]
    fn protocol_freshness_inputs_use_any_recorded_for_artifact_and_requires() {
        let protocol = make_protocol(
            "publish",
            &["request"],
            &[],
            &["published"],
            &[],
            TriggerCondition::OnArtifact {
                name: "request".into(),
            },
        );

        let inputs = protocol_freshness_inputs(&protocol);

        assert_eq!(
            inputs.get("request"),
            Some(&FreshnessInputMode::AnyRecorded)
        );
    }

    #[test]
    fn protocol_relevant_inputs_changed_ignores_other_work_units() {
        let protocol = make_protocol(
            "review",
            &["doc"],
            &[],
            &[],
            &[],
            TriggerCondition::OnArtifact { name: "doc".into() },
        );
        let scan_result = crate::ScanResult {
            modified: vec![crate::ArtifactRef {
                artifact_type: "doc".into(),
                instance_id: "b".into(),
                path: Path::new("doc/b.json").to_path_buf(),
                work_unit: Some("wu-b".into()),
            }],
            ..Default::default()
        };

        assert!(!protocol_relevant_inputs_changed(
            &protocol,
            Some("wu-a"),
            &scan_result,
        ));
        assert!(protocol_relevant_inputs_changed(
            &protocol,
            Some("wu-b"),
            &scan_result,
        ));
        assert!(protocol_relevant_inputs_changed(
            &protocol,
            None,
            &scan_result
        ));
    }

    #[test]
    fn protocol_relevant_inputs_changed_counts_invalid_and_malformed_observations() {
        let protocol = make_protocol(
            "repair",
            &[],
            &[],
            &[],
            &[],
            TriggerCondition::OnInvalid {
                name: "report".into(),
            },
        );
        let invalid_scan = crate::ScanResult {
            invalid: vec![crate::InvalidArtifact {
                artifact_type: "report".into(),
                instance_id: "bad".into(),
                path: Path::new("report/bad.json").to_path_buf(),
                work_unit: Some("wu-a".into()),
                violations: Vec::new(),
            }],
            ..Default::default()
        };
        let malformed_scan = crate::ScanResult {
            malformed: vec![crate::MalformedArtifact {
                artifact_type: "report".into(),
                instance_id: "bad".into(),
                path: Path::new("report/bad.json").to_path_buf(),
                work_unit: None,
                error: "oops".into(),
            }],
            ..Default::default()
        };

        assert!(protocol_relevant_inputs_changed(
            &protocol,
            Some("wu-a"),
            &invalid_scan,
        ));
        assert!(protocol_relevant_inputs_changed(
            &protocol,
            Some("wu-a"),
            &malformed_scan,
        ));
    }

    #[test]
    fn artifact_only_protocol_evaluated_once_unscoped() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc"]);
        store
            .record("doc", "d1", Path::new("d1.json"), &json!({"title": "D"}))
            .unwrap();

        let protocol = make_protocol(
            "ground",
            &[],
            &[],
            &[],
            &[],
            TriggerCondition::OnArtifact { name: "doc".into() },
        );

        let candidates =
            discover_ready_candidates(&[protocol], &store, &["ground"], &HashSet::new());

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

        let mut protocol = make_protocol(
            "implement",
            &["constraints"],
            &[],
            &["implementation"],
            &[],
            TriggerCondition::OnArtifact {
                name: "constraints".into(),
            },
        );
        protocol.scoped = true;

        let wu_a = discover_ready_candidates_scoped(
            &[protocol.clone()],
            &store,
            &["implement"],
            &HashSet::new(),
            "wu-a",
        );
        let wu_b = discover_ready_candidates_scoped(
            &[protocol],
            &store,
            &["implement"],
            &HashSet::new(),
            "wu-b",
        );

        assert_eq!(wu_a.len(), 1);
        assert_eq!(wu_a[0].protocol_name, "implement");
        assert_eq!(wu_a[0].work_unit, Some("wu-a".into()));
        assert_eq!(wu_b.len(), 1);
        assert_eq!(wu_b[0].work_unit, Some("wu-b".into()));
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

        let mut protocol = make_protocol(
            "implement",
            &["constraints"],
            &[],
            &["implementation"],
            &[],
            TriggerCondition::OnArtifact {
                name: "constraints".into(),
            },
        );
        protocol.scoped = true;

        let candidates = discover_ready_candidates_scoped(
            &[protocol],
            &store,
            &["implement"],
            &HashSet::new(),
            "wu-a",
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

        let mut protocol = make_protocol(
            "implement",
            &["constraints"],
            &[],
            &["implementation"],
            &[],
            TriggerCondition::OnArtifact {
                name: "constraints".into(),
            },
        );
        protocol.scoped = true;

        let candidates = discover_ready_candidates_scoped(
            &[protocol],
            &store,
            &["implement"],
            &HashSet::new(),
            "wu-a",
        );

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].protocol_name, "implement");
        assert_eq!(candidates[0].work_unit, Some("wu-a".into()));
    }

    #[test]
    fn invalid_sibling_reopens_on_artifact_protocol() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["request", "published"]);
        store
            .record_with_timestamp(
                "request",
                "good",
                Path::new("good.json"),
                &json!({"title": "good", "work_unit": "wu-a"}),
                1000,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "published",
                "good",
                Path::new("published.json"),
                &json!({"title": "published", "work_unit": "wu-a"}),
                2000,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "request",
                "bad",
                Path::new("bad.json"),
                &json!({"work_unit": "wu-a"}),
                3000,
            )
            .unwrap();

        let mut protocol = make_protocol(
            "publish",
            &["request"],
            &[],
            &["published"],
            &[],
            TriggerCondition::OnArtifact {
                name: "request".into(),
            },
        );
        protocol.scoped = true;

        let classified = classify_candidates_scoped(
            std::slice::from_ref(&protocol),
            &store,
            &["publish"],
            &HashSet::new(),
            "wu-a",
        );

        assert_eq!(classified.len(), 1);
        assert_eq!(classified[0].protocol_name, "publish");
        assert_eq!(classified[0].work_unit, Some("wu-a".into()));
        assert!(matches!(&classified[0].status, CandidateStatus::Ready));

        let candidates = discover_ready_candidates_scoped(
            &[protocol],
            &store,
            &["publish"],
            &HashSet::new(),
            "wu-a",
        );
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].protocol_name, "publish");
        assert_eq!(candidates[0].work_unit, Some("wu-a".into()));
    }

    #[test]
    fn unscoped_valid_artifact_does_not_promote_invalid_scoped_sibling() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["request", "published"]);
        store
            .record(
                "request",
                "shared",
                Path::new("shared.json"),
                &json!({"title": "shared"}),
            )
            .unwrap();
        store
            .record(
                "request",
                "bad",
                Path::new("bad.json"),
                &json!({"work_unit": "wu-a"}),
            )
            .unwrap();

        let protocol = make_protocol(
            "publish",
            &["request"],
            &[],
            &["published"],
            &[],
            TriggerCondition::OnArtifact {
                name: "request".into(),
            },
        );

        let classified = classify_candidates(
            std::slice::from_ref(&protocol),
            &store,
            &["publish"],
            &HashSet::new(),
            EvaluationScope::Unscoped,
        );

        assert_eq!(classified.len(), 1);
        assert_eq!(classified[0].protocol_name, "publish");
        assert_eq!(classified[0].work_unit, None);
        assert!(matches!(&classified[0].status, CandidateStatus::Ready));

        let candidates =
            discover_ready_candidates(&[protocol], &store, &["publish"], &HashSet::new());
        assert_eq!(
            candidates,
            vec![Candidate {
                protocol_name: "publish".into(),
                work_unit: None,
            }]
        );
    }

    #[test]
    fn previously_valid_sibling_becoming_invalid_reopens_on_artifact_protocol() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["request", "published"]);
        store
            .record_with_timestamp(
                "request",
                "a",
                Path::new("a.json"),
                &json!({"title": "a", "work_unit": "wu-a"}),
                1000,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "request",
                "b",
                Path::new("b.json"),
                &json!({"title": "b", "work_unit": "wu-a"}),
                1500,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "published",
                "good",
                Path::new("published.json"),
                &json!({"title": "published", "work_unit": "wu-a"}),
                2000,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "request",
                "b",
                Path::new("b.json"),
                &json!({"work_unit": "wu-a"}),
                3000,
            )
            .unwrap();

        let mut protocol = make_protocol(
            "publish",
            &["request"],
            &[],
            &["published"],
            &[],
            TriggerCondition::OnArtifact {
                name: "request".into(),
            },
        );
        protocol.scoped = true;

        let classified = classify_candidates_scoped(
            std::slice::from_ref(&protocol),
            &store,
            &["publish"],
            &HashSet::new(),
            "wu-a",
        );

        assert_eq!(classified.len(), 1);
        assert_eq!(classified[0].protocol_name, "publish");
        assert_eq!(classified[0].work_unit, Some("wu-a".into()));
        assert!(matches!(&classified[0].status, CandidateStatus::Ready));

        let candidates = discover_ready_candidates_scoped(
            &[protocol],
            &store,
            &["publish"],
            &HashSet::new(),
            "wu-a",
        );
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].protocol_name, "publish");
        assert_eq!(candidates[0].work_unit, Some("wu-a".into()));
    }

    #[test]
    fn discover_ready_candidates_keeps_on_change_freshness_per_work_unit() {
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
                "constraints",
                "b1",
                Path::new("b1.json"),
                &json!({"title": "B", "work_unit": "wu-b"}),
                1000,
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
        store
            .record_with_timestamp(
                "implementation",
                "b1",
                Path::new("impl-b1.json"),
                &json!({"title": "impl-B", "work_unit": "wu-b"}),
                2000,
            )
            .unwrap();

        let mut protocol = make_protocol(
            "implement",
            &["constraints"],
            &[],
            &["implementation"],
            &[],
            TriggerCondition::OnChange {
                name: "constraints".into(),
            },
        );
        protocol.scoped = true;

        let candidates = discover_ready_candidates_scoped(
            &[protocol],
            &store,
            &["implement"],
            &HashSet::new(),
            "wu-a",
        );

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].protocol_name, "implement");
        assert_eq!(candidates[0].work_unit, Some("wu-a".into()));
    }

    #[test]
    fn accepts_artifacts_do_not_affect_currentness() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(
            &tmp.path().join("s"),
            vec!["constraints", "prior-art", "implementation"],
        );
        store
            .record_with_timestamp(
                "constraints",
                "a1",
                Path::new("constraints-a1.json"),
                &json!({"title": "A", "work_unit": "wu-a"}),
                1000,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "prior-art",
                "a1",
                Path::new("prior-art-a1.json"),
                &json!({"title": "optional", "work_unit": "wu-a"}),
                3000,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "implementation",
                "a1",
                Path::new("impl-a1.json"),
                &json!({"title": "impl-A", "work_unit": "wu-a"}),
                2000,
            )
            .unwrap();

        let protocol = make_protocol(
            "implement",
            &["constraints"],
            &["prior-art"],
            &["implementation"],
            &[],
            TriggerCondition::OnArtifact {
                name: "constraints".into(),
            },
        );

        let candidates =
            discover_ready_candidates(&[protocol], &store, &["implement"], &HashSet::new());

        assert!(candidates.is_empty());
    }

    #[test]
    fn may_produce_only_protocols_are_not_suppressed() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["constraints", "notes"]);
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
                "notes",
                "old-note",
                Path::new("note.json"),
                &json!({"title": "old", "work_unit": "wu-a"}),
                500,
            )
            .unwrap();

        let mut protocol = make_protocol(
            "implement",
            &["constraints"],
            &[],
            &[],
            &["notes"],
            TriggerCondition::OnArtifact {
                name: "constraints".into(),
            },
        );
        protocol.scoped = true;

        let candidates = discover_ready_candidates_scoped(
            &[protocol],
            &store,
            &["implement"],
            &HashSet::new(),
            "wu-a",
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

        let candidates =
            discover_ready_candidates(&[protocol], &store, &["implement"], &HashSet::new());

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

        let candidates =
            discover_ready_candidates(&[protocol], &store, &["implement"], &HashSet::new());

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
                    TriggerCondition::OnArtifact {
                        name: "constraints".into(),
                    },
                ],
            },
        );

        let candidates =
            discover_ready_candidates(&[protocol], &store, &["implement"], &HashSet::new());

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

        // AnyOf(on_artifact("constraints"), on_change("constraints"))
        // on_artifact fires, but constraints haven't changed since completion.
        // The trigger is satisfied (via on_artifact), but no on_change was satisfied,
        // so suppression should apply.
        let protocol = make_protocol(
            "implement",
            &["constraints"],
            &[],
            &["implementation"],
            &[],
            TriggerCondition::AnyOf {
                conditions: vec![
                    TriggerCondition::OnArtifact {
                        name: "constraints".into(),
                    },
                    TriggerCondition::OnChange {
                        name: "constraints".into(),
                    },
                ],
            },
        );

        let candidates =
            discover_ready_candidates(&[protocol], &store, &["implement"], &HashSet::new());

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

        // any_of(all_of(on_change(constraints), on_invalid(constraints)), on_artifact(constraints))
        // The all_of branch is NOT satisfied (constraints is valid, so on_invalid
        // fails), so on_change in it should not count. The trigger fires via
        // on_artifact("constraints") only.
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
                            TriggerCondition::OnInvalid {
                                name: "constraints".into(),
                            },
                        ],
                    },
                    TriggerCondition::OnArtifact {
                        name: "constraints".into(),
                    },
                ],
            },
        );

        let candidates =
            discover_ready_candidates(&[protocol], &store, &["implement"], &HashSet::new());

        // Must be suppressed: the on_change is in an unsatisfied branch,
        // the trigger fires via on_artifact("constraints"), and postconditions pass.
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

        let candidates = discover_ready_candidates(&[protocol], &store, &["implement"], &partial);

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

        let candidates = discover_ready_candidates(&[protocol], &store, &["fix-lint"], &partial);

        assert!(candidates.is_empty());
    }

    #[test]
    fn partially_scanned_output_type_does_not_globally_skip_candidate_discovery() {
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
                "constraints",
                "b1",
                Path::new("b1.json"),
                &json!({"title": "B", "work_unit": "wu-b"}),
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
        store
            .record(
                "implementation",
                "b1",
                Path::new("impl-b1.json"),
                &json!({"title": "impl-B", "work_unit": "wu-b"}),
            )
            .unwrap();

        let partial = HashSet::from(["implementation".to_string()]);

        let mut protocol = make_protocol(
            "implement",
            &["constraints"],
            &[],
            &["implementation"],
            &[],
            TriggerCondition::OnArtifact {
                name: "constraints".into(),
            },
        );
        protocol.scoped = true;

        let wu_a = discover_ready_candidates_scoped(
            &[protocol.clone()],
            &store,
            &["implement"],
            &partial,
            "wu-a",
        );
        let wu_b =
            discover_ready_candidates_scoped(&[protocol], &store, &["implement"], &partial, "wu-b");

        assert_eq!(wu_a.len(), 1);
        assert_eq!(wu_a[0].work_unit, Some("wu-a".into()));
        assert_eq!(wu_b.len(), 1);
        assert_eq!(wu_b[0].work_unit, Some("wu-b".into()));
    }

    #[test]
    fn partially_scanned_output_type_does_not_affect_non_on_change_trigger_gate() {
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

        let partial = HashSet::from(["implementation".to_string()]);
        let mut protocol = make_protocol(
            "implement",
            &["constraints"],
            &[],
            &["implementation"],
            &[],
            TriggerCondition::OnArtifact {
                name: "constraints".into(),
            },
        );
        protocol.scoped = true;

        let candidates =
            discover_ready_candidates_scoped(&[protocol], &store, &["implement"], &partial, "wu-a");

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].protocol_name, "implement");
        assert_eq!(candidates[0].work_unit, Some("wu-a".into()));
    }

    #[test]
    fn partially_scanned_on_change_output_stays_ready() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc", "reviewed"]);
        store
            .record(
                "doc",
                "draft",
                Path::new("draft.json"),
                &json!({"title": "Draft"}),
            )
            .unwrap();
        store
            .record_with_timestamp(
                "reviewed",
                "done",
                Path::new("done.json"),
                &json!({"title": "Done"}),
                2000,
            )
            .unwrap();

        let partial = HashSet::from(["reviewed".to_string()]);
        let protocol = make_protocol(
            "review",
            &[],
            &[],
            &["reviewed"],
            &[],
            TriggerCondition::OnChange { name: "doc".into() },
        );

        let classified = classify_candidates(
            std::slice::from_ref(&protocol),
            &store,
            &["review"],
            &partial,
            EvaluationScope::Unscoped,
        );

        assert_eq!(classified.len(), 1);
        assert!(matches!(classified[0].status, CandidateStatus::Ready));
        assert!(classified[0].trigger_satisfied);
        assert!(classified[0].scan_trust.trusted);
        assert!(classified[0].scan_trust.incomplete_types.is_empty());

        let candidates = discover_ready_candidates(&[protocol], &store, &["review"], &partial);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].protocol_name, "review");
        assert_eq!(candidates[0].work_unit, None);
    }

    #[test]
    fn partially_scanned_on_change_output_unsuppresses_all_work_units() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["doc", "reviewed"]);
        store
            .record_with_timestamp(
                "doc",
                "a",
                Path::new("doc-a.json"),
                &json!({"title": "Draft A", "work_unit": "wu-a"}),
                1000,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "doc",
                "b",
                Path::new("doc-b.json"),
                &json!({"title": "Draft B", "work_unit": "wu-b"}),
                1000,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "reviewed",
                "a",
                Path::new("reviewed-a.json"),
                &json!({"title": "Done A", "work_unit": "wu-a"}),
                2000,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "reviewed",
                "b",
                Path::new("reviewed-b.json"),
                &json!({"title": "Done B", "work_unit": "wu-b"}),
                2000,
            )
            .unwrap();
        store.mark_instance_scan_gap("reviewed", "a");

        let partial = HashSet::from(["reviewed".to_string()]);
        let mut protocol = make_protocol(
            "review",
            &[],
            &[],
            &["reviewed"],
            &[],
            TriggerCondition::OnChange { name: "doc".into() },
        );
        protocol.scoped = true;

        let classified_a = classify_candidates_scoped(
            std::slice::from_ref(&protocol),
            &store,
            &["review"],
            &partial,
            "wu-a",
        );
        let classified_b = classify_candidates_scoped(
            std::slice::from_ref(&protocol),
            &store,
            &["review"],
            &partial,
            "wu-b",
        );

        assert_eq!(classified_a.len(), 1);
        assert!(matches!(classified_a[0].status, CandidateStatus::Ready));
        assert_eq!(classified_b.len(), 1);
        assert!(matches!(classified_b[0].status, CandidateStatus::Ready));

        let wu_a = discover_ready_candidates_scoped(
            &[protocol.clone()],
            &store,
            &["review"],
            &partial,
            "wu-a",
        );
        let wu_b =
            discover_ready_candidates_scoped(&[protocol], &store, &["review"], &partial, "wu-b");
        assert_eq!(wu_a.len(), 1);
        assert_eq!(wu_a[0].work_unit, Some("wu-a".into()));
        assert_eq!(wu_b.len(), 1);
        assert_eq!(wu_b[0].work_unit, Some("wu-b".into()));
    }

    #[test]
    fn partially_scanned_optional_output_does_not_skip_completed_suppression() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(
            &tmp.path().join("s"),
            vec!["constraints", "implementation", "notes"],
        );
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

        let partial = HashSet::from(["notes".to_string()]);

        let mut protocol = make_protocol(
            "implement",
            &["constraints"],
            &[],
            &["implementation"],
            &["notes"],
            TriggerCondition::OnArtifact {
                name: "constraints".into(),
            },
        );
        protocol.scoped = true;

        let candidates =
            discover_ready_candidates_scoped(&[protocol], &store, &["implement"], &partial, "wu-a");

        assert!(candidates.is_empty());
    }

    #[test]
    fn present_partially_scanned_optional_output_skips_completed_suppression() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(
            &tmp.path().join("s"),
            vec!["constraints", "implementation", "notes"],
        );
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
        store
            .record(
                "notes",
                "a1",
                Path::new("notes-a1.json"),
                &json!({"title": "optional", "work_unit": "wu-a"}),
            )
            .unwrap();

        let partial = HashSet::from(["notes".to_string()]);
        let mut protocol = make_protocol(
            "implement",
            &["constraints"],
            &[],
            &["implementation"],
            &["notes"],
            TriggerCondition::OnArtifact {
                name: "constraints".into(),
            },
        );
        protocol.scoped = true;

        let candidates =
            discover_ready_candidates_scoped(&[protocol], &store, &["implement"], &partial, "wu-a");

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].protocol_name, "implement");
        assert_eq!(candidates[0].work_unit, Some("wu-a".into()));
    }

    #[test]
    fn unsatisfied_trigger_not_candidate() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp.path().join("s"), vec!["doc", "missing"]);

        let protocol = make_protocol(
            "ground",
            &[],
            &[],
            &["doc"],
            &[],
            TriggerCondition::OnArtifact {
                name: "missing".into(),
            },
        );

        let candidates =
            discover_ready_candidates(&[protocol], &store, &["ground"], &HashSet::new());

        assert!(candidates.is_empty());
    }

    #[test]
    fn preconditions_fail_not_candidate() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["constraints", "implementation"]);
        // constraints is required but missing — only an implementation instance exists.
        store
            .record(
                "implementation",
                "a1",
                Path::new("impl-a1.json"),
                &json!({"title": "impl-A"}),
            )
            .unwrap();

        let protocol = make_protocol(
            "implement",
            &["constraints"],
            &[],
            &["implementation"],
            &[],
            TriggerCondition::OnArtifact {
                name: "implementation".into(),
            },
        );

        let candidates =
            discover_ready_candidates(&[protocol], &store, &["implement"], &HashSet::new());

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
        let candidates =
            discover_ready_candidates(&protocols, &store, &["beta", "alpha"], &HashSet::new());

        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].protocol_name, "beta");
        assert_eq!(candidates[1].protocol_name, "alpha");
    }

    #[test]
    fn stale_shared_outputs_keep_scoped_work_runnable() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(
            &tmp.path().join("s"),
            vec!["constraints", "implementation", "summary"],
        );
        store
            .record_with_timestamp(
                "constraints",
                "a1",
                Path::new("constraints-a1.json"),
                &json!({"title": "A", "work_unit": "wu-a"}),
                1500,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "implementation",
                "a1",
                Path::new("impl-a1.json"),
                &json!({"title": "impl-A", "work_unit": "wu-a"}),
                2000,
            )
            .unwrap();
        store
            .record_with_timestamp(
                "summary",
                "shared",
                Path::new("summary.json"),
                &json!({"title": "summary"}),
                1000,
            )
            .unwrap();

        let mut protocol = make_protocol(
            "implement",
            &["constraints"],
            &[],
            &["implementation", "summary"],
            &[],
            TriggerCondition::OnChange {
                name: "constraints".into(),
            },
        );
        protocol.scoped = true;

        let candidates = discover_ready_candidates_scoped(
            &[protocol],
            &store,
            &["implement"],
            &HashSet::new(),
            "wu-a",
        );

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].protocol_name, "implement");
        assert_eq!(candidates[0].work_unit, Some("wu-a".into()));
    }

    // --- classify_candidates tests ---

    #[test]
    fn classify_ready_candidate() {
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

        let mut protocol = make_protocol(
            "implement",
            &["constraints"],
            &[],
            &["implementation"],
            &[],
            TriggerCondition::OnArtifact {
                name: "constraints".into(),
            },
        );
        protocol.scoped = true;

        let classified = classify_candidates_scoped(
            &[protocol],
            &store,
            &["implement"],
            &HashSet::new(),
            "wu-a",
        );

        assert_eq!(classified.len(), 1);
        assert_eq!(classified[0].protocol_name, "implement");
        assert_eq!(classified[0].work_unit, Some("wu-a".into()));
        assert!(matches!(classified[0].status, CandidateStatus::Ready));
        assert!(classified[0].trigger_satisfied);
        assert!(classified[0].scan_trust.trusted);
    }

    #[test]
    fn classify_waiting_trigger_not_satisfied() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp.path().join("s"), vec!["doc", "missing"]);

        let protocol = make_protocol(
            "ground",
            &[],
            &[],
            &["doc"],
            &[],
            TriggerCondition::OnArtifact {
                name: "missing".into(),
            },
        );

        let classified = classify_candidates(
            &[protocol],
            &store,
            &["ground"],
            &HashSet::new(),
            EvaluationScope::Unscoped,
        );

        assert_eq!(classified.len(), 1);
        assert!(matches!(
            classified[0].status,
            CandidateStatus::Waiting { .. }
        ));
        assert!(!classified[0].trigger_satisfied);
        if let CandidateStatus::Waiting {
            unsatisfied_conditions,
        } = &classified[0].status
        {
            assert!(!unsatisfied_conditions.is_empty());
        }
    }

    #[test]
    fn classify_candidates_respects_declared_scope_boundary() {
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

        let mut scoped = make_protocol(
            "implement",
            &["constraints"],
            &[],
            &["implementation"],
            &[],
            TriggerCondition::OnArtifact {
                name: "constraints".into(),
            },
        );
        scoped.scoped = true;

        let unscoped = make_protocol(
            "ground",
            &[],
            &[],
            &["constraints"],
            &[],
            TriggerCondition::OnArtifact {
                name: "constraints".into(),
            },
        );

        let unscoped_only = classify_candidates(
            &[scoped.clone(), unscoped.clone()],
            &store,
            &["ground", "implement"],
            &HashSet::new(),
            EvaluationScope::Unscoped,
        );
        assert_eq!(unscoped_only.len(), 1);
        assert_eq!(unscoped_only[0].protocol_name, "ground");
        assert_eq!(unscoped_only[0].work_unit, None);

        let scoped_only = classify_candidates(
            &[scoped, unscoped],
            &store,
            &["ground", "implement"],
            &HashSet::new(),
            EvaluationScope::Scoped("wu-a"),
        );
        assert_eq!(scoped_only.len(), 1);
        assert_eq!(scoped_only[0].protocol_name, "implement");
        assert_eq!(scoped_only[0].work_unit.as_deref(), Some("wu-a"));
    }

    #[test]
    fn classify_blocked_precondition_fails() {
        let tmp = TempDir::new().unwrap();
        let mut store = make_store(&tmp.path().join("s"), vec!["constraints", "implementation"]);
        // Trigger fires on implementation, but constraints (required) is missing.
        store
            .record(
                "implementation",
                "a1",
                Path::new("impl-a1.json"),
                &json!({"title": "impl-A"}),
            )
            .unwrap();

        let protocol = make_protocol(
            "implement",
            &["constraints"],
            &[],
            &["implementation"],
            &[],
            TriggerCondition::OnArtifact {
                name: "implementation".into(),
            },
        );

        let classified = classify_candidates(
            &[protocol],
            &store,
            &["implement"],
            &HashSet::new(),
            EvaluationScope::Unscoped,
        );

        assert_eq!(classified.len(), 1);
        assert!(matches!(
            classified[0].status,
            CandidateStatus::Blocked { .. }
        ));
        assert!(classified[0].trigger_satisfied);
        if let CandidateStatus::Blocked {
            precondition_failures,
            ..
        } = &classified[0].status
        {
            assert!(!precondition_failures.is_empty());
            assert!(matches!(
                precondition_failures[0],
                ArtifactFailure::Missing { .. }
            ));
        }
    }

    #[test]
    fn classify_waiting_outputs_current() {
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

        let classified = classify_candidates(
            &[protocol],
            &store,
            &["implement"],
            &HashSet::new(),
            EvaluationScope::Unscoped,
        );

        assert_eq!(classified.len(), 1);
        assert!(matches!(
            classified[0].status,
            CandidateStatus::Waiting { .. }
        ));
        assert!(classified[0].trigger_satisfied);
        if let CandidateStatus::Waiting {
            unsatisfied_conditions,
        } = &classified[0].status
        {
            assert_eq!(unsatisfied_conditions, &["outputs are current"]);
        }
    }

    #[test]
    fn classify_blocked_scan_incomplete() {
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

        let classified = classify_candidates(
            &[protocol],
            &store,
            &["implement"],
            &partial,
            EvaluationScope::Unscoped,
        );

        assert_eq!(classified.len(), 1);
        assert!(matches!(
            classified[0].status,
            CandidateStatus::Blocked { .. }
        ));
        assert!(!classified[0].scan_trust.trusted);
        if let CandidateStatus::Blocked {
            scan_incomplete_types,
            ..
        } = &classified[0].status
        {
            assert!(scan_incomplete_types.contains(&"constraints".to_string()));
        }
    }

    #[test]
    fn classify_and_discover_agree_on_ready() {
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
        // wu-a has current output, wu-b does not
        store
            .record(
                "implementation",
                "a1",
                Path::new("impl-a1.json"),
                &json!({"title": "impl-A", "work_unit": "wu-a"}),
            )
            .unwrap();

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

        let classified = classify_candidates(
            std::slice::from_ref(&protocol),
            &store,
            &["implement"],
            &HashSet::new(),
            EvaluationScope::Unscoped,
        );
        let ready_from_classify: Vec<_> = classified
            .iter()
            .filter(|c| matches!(c.status, CandidateStatus::Ready))
            .map(|c| (c.protocol_name.clone(), c.work_unit.clone()))
            .collect();

        let ready_from_discover =
            discover_ready_candidates(&[protocol], &store, &["implement"], &HashSet::new());
        let ready_from_discover: Vec<_> = ready_from_discover
            .iter()
            .map(|c| (c.protocol_name.clone(), c.work_unit.clone()))
            .collect();

        assert_eq!(ready_from_classify, ready_from_discover);
    }
}
