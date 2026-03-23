use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

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

    let classified = libagent::classify_candidates(
        &loaded.manifest.protocols,
        &loaded.store,
        &skill_order,
        &scan_findings.affected_types,
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

        let entry = match candidate.status {
            libagent::CandidateStatus::Ready => ProtocolEntry {
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
            },
            libagent::CandidateStatus::Blocked {
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
                }
            }
            libagent::CandidateStatus::Waiting {
                unsatisfied_conditions,
            } => ProtocolEntry {
                name: candidate.protocol_name,
                work_unit: candidate.work_unit,
                status: ProtocolStatus::Waiting,
                trigger,
                inputs: Vec::new(),
                precondition_failures: Vec::new(),
                unsatisfied_conditions,
            },
        };

        match entry.status {
            ProtocolStatus::Ready => ready.push(entry),
            ProtocolStatus::Blocked => blocked.push(entry),
            ProtocolStatus::Waiting => waiting.push(entry),
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

fn display_protocol_name(entry: &ProtocolEntry) -> String {
    match &entry.work_unit {
        Some(work_unit) => format!("{} (work_unit={work_unit})", entry.name),
        None => entry.name.clone(),
    }
}
