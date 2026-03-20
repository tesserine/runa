use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use libagent::{TriggerContext, TriggerResult, enforce_preconditions, evaluate_trigger};
use serde::Serialize;

use crate::project;

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProtocolStatus {
    Ready,
    Blocked,
    Waiting,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TriggerState {
    Satisfied,
    NotSatisfied,
}

#[derive(Clone)]
pub(crate) struct ProtocolEntry {
    pub(crate) name: String,
    pub(crate) work_unit: Option<String>,
    pub(crate) status: ProtocolStatus,
    pub(crate) trigger: TriggerState,
    pub(crate) inputs: Vec<InputEntry>,
    pub(crate) precondition_failures: Vec<FailureEntry>,
    pub(crate) unsatisfied_conditions: Vec<String>,
}

#[derive(Clone, Serialize)]
pub(crate) struct ProtocolJson {
    pub(crate) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) work_unit: Option<String>,
    pub(crate) status: ProtocolStatus,
    pub(crate) trigger: TriggerState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) inputs: Option<Vec<InputJson>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) precondition_failures: Option<Vec<FailureJson>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) unsatisfied_conditions: Option<Vec<String>>,
}

#[derive(Clone, Serialize)]
pub(crate) struct InputJson {
    pub(crate) artifact_type: String,
    pub(crate) instance_id: String,
    pub(crate) path: String,
    pub(crate) relationship: &'static str,
}

#[derive(Clone, Serialize)]
pub(crate) struct FailureJson {
    pub(crate) artifact_type: String,
    pub(crate) reason: &'static str,
}

#[derive(Clone)]
pub(crate) struct InputEntry {
    pub(crate) artifact_type: String,
    pub(crate) instance_id: String,
    pub(crate) path: String,
    pub(crate) relationship: &'static str,
}

#[derive(Clone)]
pub(crate) struct FailureEntry {
    pub(crate) artifact_type: String,
    pub(crate) reason: &'static str,
}

pub(crate) struct ScanFindings {
    pub(crate) affected_types: HashSet<String>,
    pub(crate) warnings: Vec<String>,
}

struct TriggerEvaluation {
    satisfied: bool,
    trusted: bool,
    scan_types: Vec<String>,
}

pub(crate) struct EvaluatedProtocols {
    pub(crate) cycle: Option<libagent::CycleError>,
    pub(crate) ready: Vec<ProtocolEntry>,
    pub(crate) blocked: Vec<ProtocolEntry>,
    pub(crate) waiting: Vec<ProtocolEntry>,
}

impl EvaluatedProtocols {
    pub(crate) fn ordered_entries(&self) -> impl Iterator<Item = &ProtocolEntry> {
        self.ready
            .iter()
            .chain(self.blocked.iter())
            .chain(self.waiting.iter())
    }

    pub(crate) fn json_protocols(&self) -> Vec<ProtocolJson> {
        self.ordered_entries().cloned().map(protocol_json).collect()
    }
}

fn failure_entries_from_types(artifact_types: Vec<String>) -> Vec<FailureEntry> {
    artifact_types
        .into_iter()
        .map(|artifact_type| FailureEntry {
            artifact_type,
            reason: "scan_incomplete",
        })
        .collect()
}

fn append_unique_failures(target: &mut Vec<FailureEntry>, additional: Vec<FailureEntry>) {
    for failure in additional {
        if !target.iter().any(|existing| {
            existing.artifact_type == failure.artifact_type && existing.reason == failure.reason
        }) {
            target.push(failure);
        }
    }
}

fn precondition_scan_failures(
    protocol: &libagent::ProtocolDeclaration,
    affected_types: &HashSet<String>,
) -> Vec<FailureEntry> {
    let mut failures = Vec::new();
    for artifact_type in protocol.requires.iter().chain(protocol.produces.iter()) {
        if affected_types.contains(artifact_type.as_str())
            && !failures
                .iter()
                .any(|failure: &FailureEntry| failure.artifact_type == *artifact_type)
        {
            failures.push(FailureEntry {
                artifact_type: artifact_type.clone(),
                reason: "scan_incomplete",
            });
        }
    }
    failures
}

pub(crate) fn collect_scan_findings(
    scan_result: &libagent::ScanResult,
    workspace_dir: &Path,
) -> ScanFindings {
    let mut affected_types: HashSet<String> = scan_result
        .partially_scanned_types
        .iter()
        .map(|partial| partial.artifact_type.clone())
        .collect();
    let mut warnings = Vec::new();

    for partial in &scan_result.partially_scanned_types {
        warnings.push(format!(
            "artifact type '{}' was only partially scanned: {} unreadable entr{}",
            partial.artifact_type,
            partial.unreadable_entries,
            if partial.unreadable_entries == 1 {
                "y"
            } else {
                "ies"
            }
        ));
    }

    for unreadable in &scan_result.unreadable {
        if let Some(artifact_type) = artifact_type_from_path(&unreadable.path, workspace_dir) {
            affected_types.insert(artifact_type);
        }
    }

    ScanFindings {
        affected_types,
        warnings,
    }
}

pub(crate) fn evaluate_protocols(
    loaded: &project::LoadedProject,
    working_dir: &Path,
    scan_findings: &ScanFindings,
    active_signals: &HashSet<String>,
) -> EvaluatedProtocols {
    let (skill_order, cycle) = match loaded.graph.topological_order() {
        Ok(order) => (order, None),
        Err(cycle) => {
            eprintln!("warning: {cycle}");
            let cycle_participants: HashSet<&str> =
                cycle.path.iter().map(|name| name.as_str()).collect();
            let mut order = loaded
                .graph
                .topological_order_excluding(&cycle_participants);
            order.extend(
                loaded
                    .manifest
                    .protocols
                    .iter()
                    .map(|protocol| protocol.name.as_str())
                    .filter(|name| cycle_participants.contains(name)),
            );
            (order, Some(cycle))
        }
    };

    let skill_map: HashMap<&str, &libagent::ProtocolDeclaration> = loaded
        .manifest
        .protocols
        .iter()
        .map(|protocol| (protocol.name.as_str(), protocol))
        .collect();

    let ready_candidates = libagent::discover_ready_candidates(
        &loaded.manifest.protocols,
        &loaded.store,
        active_signals,
        &skill_order,
        &scan_findings.affected_types,
    );
    let ready_set: HashSet<(String, Option<String>)> = ready_candidates
        .iter()
        .map(|candidate| (candidate.protocol_name.clone(), candidate.work_unit.clone()))
        .collect();

    let mut ready: Vec<ProtocolEntry> = ready_candidates
        .iter()
        .map(|candidate| {
            let protocol = skill_map
                .get(candidate.protocol_name.as_str())
                .expect("ready candidate protocol must exist in manifest");
            ProtocolEntry {
                name: protocol.name.clone(),
                work_unit: candidate.work_unit.clone(),
                status: ProtocolStatus::Ready,
                trigger: TriggerState::Satisfied,
                inputs: collect_inputs(
                    protocol,
                    &loaded.store,
                    working_dir,
                    &scan_findings.affected_types,
                    candidate.work_unit.as_deref(),
                ),
                precondition_failures: Vec::new(),
                unsatisfied_conditions: Vec::new(),
            }
        })
        .collect();
    let mut blocked = Vec::new();
    let mut waiting = Vec::new();

    for name in skill_order {
        let Some(protocol) = skill_map.get(name) else {
            continue;
        };

        let protocol_scan_failures =
            failure_entries_from_types(libagent::selection::protocol_scan_incomplete_types(
                protocol,
                &scan_findings.affected_types,
            ));
        let readiness_scan_failures =
            precondition_scan_failures(protocol, &scan_findings.affected_types);

        for work_unit in libagent::selection::protocol_work_units(
            protocol,
            &loaded.store,
            &scan_findings.affected_types,
        ) {
            if ready_set.contains(&(protocol.name.clone(), work_unit.clone())) {
                continue;
            }

            let context = TriggerContext {
                store: &loaded.store,
                active_signals,
                work_unit: work_unit.as_deref(),
            };
            let trigger_eval = evaluate_trigger_trust(
                &protocol.trigger,
                protocol,
                &context,
                &scan_findings.affected_types,
            );
            let trigger_state = if trigger_eval.satisfied {
                TriggerState::Satisfied
            } else {
                TriggerState::NotSatisfied
            };
            let scan_failures =
                scan_incomplete_failures(protocol, scan_findings, &trigger_eval.scan_types);

            let entry = if trigger_eval.satisfied {
                let mut precondition_failures = protocol_scan_failures.clone();
                append_unique_failures(&mut precondition_failures, scan_failures);
                if let Err(err) =
                    enforce_preconditions(protocol, &loaded.store, work_unit.as_deref())
                {
                    append_unique_failures(
                        &mut precondition_failures,
                        err.failures.iter().map(failure_entry).collect(),
                    );
                }

                if precondition_failures.is_empty() {
                    ProtocolEntry {
                        name: protocol.name.clone(),
                        work_unit: work_unit.clone(),
                        status: ProtocolStatus::Waiting,
                        trigger: TriggerState::Satisfied,
                        inputs: Vec::new(),
                        precondition_failures: Vec::new(),
                        unsatisfied_conditions: vec!["outputs are current".to_string()],
                    }
                } else {
                    ProtocolEntry {
                        name: protocol.name.clone(),
                        work_unit: work_unit.clone(),
                        status: ProtocolStatus::Blocked,
                        trigger: TriggerState::Satisfied,
                        inputs: Vec::new(),
                        precondition_failures,
                        unsatisfied_conditions: Vec::new(),
                    }
                }
            } else if scan_failures.is_empty() && readiness_scan_failures.is_empty() {
                ProtocolEntry {
                    name: protocol.name.clone(),
                    work_unit: work_unit.clone(),
                    status: ProtocolStatus::Waiting,
                    trigger: trigger_state,
                    inputs: Vec::new(),
                    precondition_failures: Vec::new(),
                    unsatisfied_conditions: collect_unsatisfied_conditions(
                        &protocol.trigger,
                        protocol,
                        &context,
                    ),
                }
            } else {
                let mut precondition_failures = readiness_scan_failures.clone();
                append_unique_failures(&mut precondition_failures, scan_failures);
                if let Err(err) =
                    enforce_preconditions(protocol, &loaded.store, work_unit.as_deref())
                {
                    append_unique_failures(
                        &mut precondition_failures,
                        err.failures.iter().map(failure_entry).collect(),
                    );
                }
                ProtocolEntry {
                    name: protocol.name.clone(),
                    work_unit: work_unit.clone(),
                    status: ProtocolStatus::Blocked,
                    trigger: trigger_state,
                    inputs: Vec::new(),
                    precondition_failures,
                    unsatisfied_conditions: Vec::new(),
                }
            };

            match entry.status {
                ProtocolStatus::Ready => ready.push(entry),
                ProtocolStatus::Blocked => blocked.push(entry),
                ProtocolStatus::Waiting => waiting.push(entry),
            }
        }
    }

    EvaluatedProtocols {
        cycle,
        ready,
        blocked,
        waiting,
    }
}

pub(crate) fn print_group(label: &str, entries: &[ProtocolEntry]) {
    println!("{label}:");
    if entries.is_empty() {
        println!("  (none)");
        return;
    }

    for entry in entries {
        println!("  {}", display_protocol_name(entry));
        match entry.status {
            ProtocolStatus::Ready => {
                for input in &entry.inputs {
                    println!(
                        "    - {}/{} ({})",
                        input.artifact_type, input.instance_id, input.relationship
                    );
                }
            }
            ProtocolStatus::Blocked => {
                for failure in &entry.precondition_failures {
                    println!("    - {} ({})", failure.artifact_type, failure.reason);
                }
            }
            ProtocolStatus::Waiting => {
                for condition in &entry.unsatisfied_conditions {
                    println!("    - {condition}");
                }
            }
        }
    }
}

fn collect_inputs(
    protocol: &libagent::ProtocolDeclaration,
    store: &libagent::ArtifactStore,
    working_dir: &Path,
    affected_types: &HashSet<String>,
    work_unit: Option<&str>,
) -> Vec<InputEntry> {
    let mut inputs = Vec::new();

    for artifact_type in &protocol.requires {
        if affected_types.contains(artifact_type) {
            continue;
        }
        for (instance_id, state) in store.instances_of(artifact_type, work_unit) {
            if matches!(state.status, libagent::ValidationStatus::Valid) {
                inputs.push(InputEntry {
                    artifact_type: artifact_type.clone(),
                    instance_id: instance_id.to_string(),
                    path: display_path(&state.path, working_dir),
                    relationship: "requires",
                });
            }
        }
    }

    for artifact_type in &protocol.accepts {
        if affected_types.contains(artifact_type) {
            continue;
        }
        for (instance_id, state) in store.instances_of(artifact_type, work_unit) {
            if matches!(state.status, libagent::ValidationStatus::Valid) {
                inputs.push(InputEntry {
                    artifact_type: artifact_type.clone(),
                    instance_id: instance_id.to_string(),
                    path: display_path(&state.path, working_dir),
                    relationship: "accepts",
                });
            }
        }
    }

    inputs
}

fn display_path(path: &Path, working_dir: &Path) -> String {
    match path.strip_prefix(working_dir) {
        Ok(relative) => relative.display().to_string(),
        Err(_) => path.display().to_string(),
    }
}

fn failure_entry(failure: &libagent::ArtifactFailure) -> FailureEntry {
    match failure {
        libagent::ArtifactFailure::Missing { artifact_type, .. } => FailureEntry {
            artifact_type: artifact_type.clone(),
            reason: "missing",
        },
        libagent::ArtifactFailure::Invalid { artifact_type, .. } => FailureEntry {
            artifact_type: artifact_type.clone(),
            reason: "invalid",
        },
        libagent::ArtifactFailure::Stale { artifact_type, .. } => FailureEntry {
            artifact_type: artifact_type.clone(),
            reason: "stale",
        },
    }
}

fn scan_incomplete_failures(
    protocol: &libagent::ProtocolDeclaration,
    scan_findings: &ScanFindings,
    trigger_scan_types: &[String],
) -> Vec<FailureEntry> {
    let mut artifact_types = trigger_scan_types.to_vec();

    for artifact_type in &protocol.requires {
        if scan_findings
            .affected_types
            .contains(artifact_type.as_str())
            && !artifact_types.contains(artifact_type)
        {
            artifact_types.push(artifact_type.clone());
        }
    }

    artifact_types
        .into_iter()
        .map(|artifact_type| FailureEntry {
            artifact_type,
            reason: "scan_incomplete",
        })
        .collect()
}

fn evaluate_trigger_trust(
    condition: &libagent::TriggerCondition,
    protocol: &libagent::ProtocolDeclaration,
    context: &TriggerContext<'_>,
    affected_types: &HashSet<String>,
) -> TriggerEvaluation {
    match condition {
        libagent::TriggerCondition::OnArtifact { name } => primitive_trigger_eval(
            condition,
            protocol,
            context,
            affected_types.contains(name.as_str()),
            !has_visible_defect(context.store, name),
            true,
            Some(name.clone()),
        ),
        libagent::TriggerCondition::OnInvalid { name } => primitive_trigger_eval(
            condition,
            protocol,
            context,
            affected_types.contains(name.as_str()),
            true,
            false,
            Some(name.clone()),
        ),
        libagent::TriggerCondition::OnChange { name } => {
            on_change_trigger_eval(condition, protocol, context, name, affected_types)
        }
        libagent::TriggerCondition::OnSignal { .. } => {
            primitive_trigger_eval(condition, protocol, context, false, false, false, None)
        }
        libagent::TriggerCondition::AllOf { conditions } => {
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
        libagent::TriggerCondition::AnyOf { conditions } => {
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
    condition: &libagent::TriggerCondition,
    protocol: &libagent::ProtocolDeclaration,
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
    condition: &libagent::TriggerCondition,
    protocol: &libagent::ProtocolDeclaration,
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

    let affected_outputs: Vec<String> = protocol
        .produces
        .iter()
        .filter(|artifact_type| affected_types.contains(artifact_type.as_str()))
        .cloned()
        .collect();
    if !affected_outputs.is_empty() {
        trusted = false;
        append_unique(&mut scan_types, affected_outputs);
    }

    TriggerEvaluation {
        satisfied,
        trusted,
        scan_types,
    }
}

fn has_visible_defect(store: &libagent::ArtifactStore, artifact_type: &str) -> bool {
    store
        .instances_of(artifact_type, None)
        .iter()
        .any(|(_, state)| {
            matches!(
                state.status,
                libagent::ValidationStatus::Invalid(_)
                    | libagent::ValidationStatus::Malformed(_)
                    | libagent::ValidationStatus::Stale
            )
        })
}

fn append_unique(target: &mut Vec<String>, values: Vec<String>) {
    for value in values {
        if !target.contains(&value) {
            target.push(value);
        }
    }
}

fn artifact_type_from_path(path: &Path, workspace_dir: &Path) -> Option<String> {
    let relative = path.strip_prefix(workspace_dir).ok()?;
    let mut components = relative.components();
    let first = components.next()?;
    Some(PathBuf::from(first.as_os_str()).display().to_string())
}

fn collect_unsatisfied_conditions(
    condition: &libagent::TriggerCondition,
    protocol: &libagent::ProtocolDeclaration,
    context: &TriggerContext<'_>,
) -> Vec<String> {
    match evaluate_trigger(condition, protocol, context) {
        TriggerResult::Satisfied => Vec::new(),
        TriggerResult::NotSatisfied(reason) => match condition {
            libagent::TriggerCondition::AllOf { conditions } => conditions
                .iter()
                .flat_map(|child| collect_unsatisfied_conditions(child, protocol, context))
                .collect(),
            libagent::TriggerCondition::AnyOf { conditions } => {
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

fn protocol_json(entry: ProtocolEntry) -> ProtocolJson {
    ProtocolJson {
        name: entry.name,
        work_unit: entry.work_unit,
        status: entry.status,
        trigger: entry.trigger,
        inputs: if entry.inputs.is_empty() {
            None
        } else {
            Some(
                entry
                    .inputs
                    .into_iter()
                    .map(|input| InputJson {
                        artifact_type: input.artifact_type,
                        instance_id: input.instance_id,
                        path: input.path,
                        relationship: input.relationship,
                    })
                    .collect(),
            )
        },
        precondition_failures: if entry.precondition_failures.is_empty() {
            None
        } else {
            Some(
                entry
                    .precondition_failures
                    .into_iter()
                    .map(|failure| FailureJson {
                        artifact_type: failure.artifact_type,
                        reason: failure.reason,
                    })
                    .collect(),
            )
        },
        unsatisfied_conditions: if entry.unsatisfied_conditions.is_empty() {
            None
        } else {
            Some(entry.unsatisfied_conditions)
        },
    }
}

fn display_protocol_name(entry: &ProtocolEntry) -> String {
    match &entry.work_unit {
        Some(work_unit) => format!("{} (work_unit={work_unit})", entry.name),
        None => entry.name.clone(),
    }
}
