use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::project::LoadedProject;

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolStatus {
    Ready,
    Blocked,
    Waiting,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerState {
    Satisfied,
    NotSatisfied,
}

#[derive(Clone)]
pub struct ProtocolEntry {
    pub name: String,
    pub work_unit: Option<String>,
    pub status: ProtocolStatus,
    pub trigger: TriggerState,
    pub inputs: Vec<InputEntry>,
    pub precondition_failures: Vec<FailureEntry>,
    pub unsatisfied_conditions: Vec<String>,
    pub waiting_reason: Option<crate::WaitingReason>,
}

#[derive(Clone, Serialize)]
pub struct ProtocolJson {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_unit: Option<String>,
    pub status: ProtocolStatus,
    pub trigger: TriggerState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inputs: Option<Vec<InputJson>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub precondition_failures: Option<Vec<FailureJson>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unsatisfied_conditions: Option<Vec<String>>,
}

#[derive(Clone, Serialize)]
pub struct InputJson {
    pub artifact_type: String,
    pub instance_id: String,
    pub path: String,
    pub relationship: &'static str,
}

#[derive(Clone, Serialize)]
pub struct FailureJson {
    pub artifact_type: String,
    pub reason: &'static str,
}

#[derive(Clone)]
pub struct InputEntry {
    pub artifact_type: String,
    pub instance_id: String,
    pub path: String,
    pub relationship: &'static str,
}

#[derive(Clone)]
pub struct FailureEntry {
    pub artifact_type: String,
    pub reason: &'static str,
}

pub struct ScanFindings {
    pub affected_types: HashSet<String>,
    pub warnings: Vec<String>,
}

pub struct EvaluatedProtocols {
    pub topology: crate::EvaluationTopology,
    pub cycle: Option<crate::CycleError>,
    pub ready: Vec<ProtocolEntry>,
    pub blocked: Vec<ProtocolEntry>,
    pub waiting: Vec<ProtocolEntry>,
}

#[derive(Serialize)]
pub struct StateJson<'a> {
    pub version: u32,
    pub methodology: &'a str,
    pub scan_warnings: Vec<String>,
    pub protocols: Vec<ProtocolJson>,
}

impl EvaluatedProtocols {
    pub fn ordered_entries(&self) -> impl Iterator<Item = &ProtocolEntry> {
        self.ready
            .iter()
            .chain(self.blocked.iter())
            .chain(self.waiting.iter())
    }

    pub fn json_protocols(&self) -> Vec<ProtocolJson> {
        self.ordered_entries().cloned().map(protocol_json).collect()
    }
}

pub fn collect_scan_findings(
    scan_result: &crate::ScanResult,
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

pub fn evaluate_protocols(
    loaded: &LoadedProject,
    working_dir: &Path,
    scan_findings: &ScanFindings,
    scope: crate::EvaluationScope<'_>,
) -> EvaluatedProtocols {
    let topology =
        crate::resolve_evaluation_topology(&loaded.manifest.protocols, &loaded.graph, scope);
    let cycle = topology.cycle.clone();
    let skill_order: Vec<&str> = topology.status_order.iter().map(String::as_str).collect();
    let cycle_participants: HashSet<&str> = cycle
        .as_ref()
        .map(|cycle| cycle.path.iter().map(|name| name.as_str()).collect())
        .unwrap_or_default();
    let cycle_condition = cycle.as_ref().map(ToString::to_string);

    let skill_map: HashMap<&str, &crate::ProtocolDeclaration> = loaded
        .manifest
        .protocols
        .iter()
        .map(|protocol| (protocol.name.as_str(), protocol))
        .collect();

    let classified = crate::classify_candidates(
        &loaded.manifest.protocols,
        &loaded.store,
        &skill_order,
        &scan_findings.affected_types,
        scope,
    );

    let mut ready = Vec::new();
    let mut blocked = Vec::new();
    let mut waiting = Vec::new();

    for candidate in classified {
        let Some(protocol) = skill_map.get(candidate.protocol_name.as_str()) else {
            continue;
        };

        let trigger = if candidate.trigger_satisfied {
            TriggerState::Satisfied
        } else {
            TriggerState::NotSatisfied
        };
        let in_cycle = cycle_participants.contains(candidate.protocol_name.as_str());

        let entry = match candidate.status {
            crate::CandidateStatus::Ready if in_cycle => ProtocolEntry {
                name: candidate.protocol_name,
                work_unit: candidate.work_unit,
                status: ProtocolStatus::Waiting,
                trigger,
                inputs: Vec::new(),
                precondition_failures: Vec::new(),
                unsatisfied_conditions: vec![
                    cycle_condition
                        .as_ref()
                        .expect("cycle participants must have a cycle condition")
                        .clone(),
                ],
                waiting_reason: Some(crate::WaitingReason::TriggerUnsatisfied),
            },
            crate::CandidateStatus::Ready => ProtocolEntry {
                name: candidate.protocol_name,
                work_unit: candidate.work_unit.clone(),
                status: ProtocolStatus::Ready,
                trigger,
                inputs: collect_inputs(
                    protocol,
                    &loaded.store,
                    working_dir,
                    &scan_findings.affected_types,
                    candidate.work_unit.as_deref(),
                ),
                precondition_failures: Vec::new(),
                unsatisfied_conditions: Vec::new(),
                waiting_reason: None,
            },
            crate::CandidateStatus::Blocked {
                precondition_failures,
                scan_incomplete_types,
            } => {
                let mut failures: Vec<FailureEntry> = scan_incomplete_types
                    .into_iter()
                    .map(|artifact_type| FailureEntry {
                        artifact_type,
                        reason: "scan_incomplete",
                    })
                    .collect();
                for f in &precondition_failures {
                    let fe = failure_entry(f);
                    if !failures.iter().any(|existing| {
                        existing.artifact_type == fe.artifact_type && existing.reason == fe.reason
                    }) {
                        failures.push(fe);
                    }
                }
                ProtocolEntry {
                    name: candidate.protocol_name,
                    work_unit: candidate.work_unit,
                    status: ProtocolStatus::Blocked,
                    trigger,
                    inputs: Vec::new(),
                    precondition_failures: failures,
                    unsatisfied_conditions: Vec::new(),
                    waiting_reason: None,
                }
            }
            crate::CandidateStatus::Waiting {
                waiting_reason,
                mut unsatisfied_conditions,
            } => {
                if let Some(condition) = &cycle_condition
                    && in_cycle
                    && !unsatisfied_conditions
                        .iter()
                        .any(|existing| existing == condition)
                {
                    unsatisfied_conditions.push(condition.clone());
                }
                ProtocolEntry {
                    name: candidate.protocol_name,
                    work_unit: candidate.work_unit,
                    status: ProtocolStatus::Waiting,
                    trigger,
                    inputs: Vec::new(),
                    precondition_failures: Vec::new(),
                    unsatisfied_conditions,
                    waiting_reason: Some(waiting_reason),
                }
            }
        };

        match entry.status {
            ProtocolStatus::Ready => ready.push(entry),
            ProtocolStatus::Blocked => blocked.push(entry),
            ProtocolStatus::Waiting => waiting.push(entry),
        }
    }

    EvaluatedProtocols {
        topology,
        cycle,
        ready,
        blocked,
        waiting,
    }
}

fn collect_inputs(
    protocol: &crate::ProtocolDeclaration,
    store: &crate::ArtifactStore,
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
            if matches!(state.status, crate::ValidationStatus::Valid) {
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
            if matches!(state.status, crate::ValidationStatus::Valid) {
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

fn failure_entry(failure: &crate::ArtifactFailure) -> FailureEntry {
    match failure {
        crate::ArtifactFailure::Missing { artifact_type, .. } => FailureEntry {
            artifact_type: artifact_type.clone(),
            reason: "missing",
        },
        crate::ArtifactFailure::Invalid { artifact_type, .. } => FailureEntry {
            artifact_type: artifact_type.clone(),
            reason: "invalid",
        },
        crate::ArtifactFailure::Stale { artifact_type, .. } => FailureEntry {
            artifact_type: artifact_type.clone(),
            reason: "stale",
        },
        crate::ArtifactFailure::RequiredChoiceMissing { choice, .. } => FailureEntry {
            artifact_type: choice.clone(),
            reason: "missing_choice",
        },
        crate::ArtifactFailure::RequiredChoiceConflict { choice, .. } => FailureEntry {
            artifact_type: choice.clone(),
            reason: "conflicting_choice",
        },
    }
}

fn artifact_type_from_path(path: &Path, workspace_dir: &Path) -> Option<String> {
    let relative = path.strip_prefix(workspace_dir).ok()?;
    let mut components = relative.components();
    let first = components.next()?;
    Some(PathBuf::from(first.as_os_str()).display().to_string())
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
