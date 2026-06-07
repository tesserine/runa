//! Shared session readiness, planning, and lifecycle state.
//!
//! This module is the common authority used by CLI commands and MCP session
//! mode when they need to evaluate protocol readiness or hold a current step.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::context::{ArtifactRelationship, ContextInjection};
use crate::project::LoadedProject;
use crate::{
    ArtifactFailure, EvaluationScope, ExecutionRecord, ProtocolDeclaration, ScanResult,
    ValidationStatus,
};

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

#[derive(Clone)]
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

#[derive(Debug, Clone, PartialEq)]
pub struct PlannedEntry {
    pub protocol: String,
    pub work_unit: Option<String>,
    pub trigger: String,
    pub context: ContextInjection,
    pub execution_record: ExecutionRecord,
}

pub struct ExecutionState {
    pub scan_findings: ScanFindings,
    pub evaluated: EvaluatedProtocols,
    pub planned_entries: Vec<PlannedEntry>,
}

#[derive(Debug)]
pub enum SessionError {
    Scan(crate::ScanError),
    Enforcement(crate::EnforcementError),
    Store(crate::StoreError),
    UnservableStep { protocol: String, reason: String },
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionError::Scan(error) => write!(f, "{error}"),
            SessionError::Enforcement(error) => write!(f, "{error}"),
            SessionError::Store(error) => write!(f, "{error}"),
            SessionError::UnservableStep { protocol, reason } => {
                write!(f, "protocol '{protocol}' cannot become current: {reason}")
            }
        }
    }
}

impl std::error::Error for SessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SessionError::Scan(error) => Some(error),
            SessionError::Enforcement(error) => Some(error),
            SessionError::Store(error) => Some(error),
            SessionError::UnservableStep { .. } => None,
        }
    }
}

#[derive(Clone, Serialize)]
pub struct StepSummary {
    pub protocol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_unit: Option<String>,
    pub trigger: String,
}

impl From<&PlannedEntry> for StepSummary {
    fn from(entry: &PlannedEntry) -> Self {
        Self {
            protocol: entry.protocol.clone(),
            work_unit: entry.work_unit.clone(),
            trigger: entry.trigger.clone(),
        }
    }
}

#[derive(Clone, Serialize)]
pub struct ReadinessReport {
    pub methodology: String,
    pub scan_warnings: Vec<String>,
    pub protocols: Vec<ProtocolJson>,
}

#[derive(Serialize)]
pub struct AdvanceReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub advanced_step: Option<StepSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_step: Option<StepSummary>,
    pub readiness: ReadinessReport,
}

pub struct Session {
    loaded: LoadedProject,
    working_dir: PathBuf,
    work_unit: String,
    current_step: Option<PlannedEntry>,
    last_readiness: ReadinessReport,
}

pub fn collect_scan_findings(scan_result: &ScanResult, workspace_dir: &Path) -> ScanFindings {
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
    scope: EvaluationScope<'_>,
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

    let skill_map: HashMap<&str, &ProtocolDeclaration> = loaded
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

pub fn evaluate_execution_state(
    loaded: &LoadedProject,
    working_dir: &Path,
    scan_result: &ScanResult,
    scope: EvaluationScope<'_>,
) -> ExecutionState {
    let scan_findings = collect_scan_findings(scan_result, &loaded.workspace_dir);
    let evaluated = evaluate_protocols(loaded, working_dir, &scan_findings, scope);
    let planned_entries = build_execution_plan(loaded, &scan_findings, &evaluated);

    ExecutionState {
        scan_findings,
        evaluated,
        planned_entries,
    }
}

pub fn build_execution_plan(
    loaded: &LoadedProject,
    scan_findings: &ScanFindings,
    evaluated: &EvaluatedProtocols,
) -> Vec<PlannedEntry> {
    let protocol_map: HashMap<&str, &ProtocolDeclaration> = loaded
        .manifest
        .protocols
        .iter()
        .map(|protocol| (protocol.name.as_str(), protocol))
        .collect();

    if evaluated.ready.is_empty() {
        return Vec::new();
    }

    evaluated
        .ready
        .iter()
        .map(|entry| {
            let protocol = protocol_map
                .get(entry.name.as_str())
                .expect("planned protocol must exist in manifest");
            let mut context =
                crate::context::build_context(protocol, &loaded.store, entry.work_unit.as_deref());
            context.inputs.retain(|input| {
                input.relationship == ArtifactRelationship::Requires
                    || !scan_findings
                        .affected_types
                        .contains(input.artifact_type.as_str())
            });
            PlannedEntry {
                protocol: entry.name.clone(),
                work_unit: entry.work_unit.clone(),
                trigger: protocol.trigger.to_string(),
                context,
                execution_record: crate::protocol_execution_record(
                    protocol,
                    &loaded.store,
                    entry.work_unit.as_deref(),
                    &scan_findings.affected_types,
                ),
            }
        })
        .collect()
}

impl Session {
    pub fn start<F>(
        mut loaded: LoadedProject,
        working_dir: PathBuf,
        work_unit: String,
        validate_current: F,
    ) -> Result<Self, SessionError>
    where
        F: Fn(&PlannedEntry, &LoadedProject) -> Result<(), String>,
    {
        let scan_result =
            crate::scan(&loaded.workspace_dir, &mut loaded.store).map_err(SessionError::Scan)?;
        crate::validate_scoped_work_unit(&loaded.store, &work_unit).map_err(|error| {
            SessionError::UnservableStep {
                protocol: "<session>".to_string(),
                reason: error.to_string(),
            }
        })?;
        let state = evaluate_execution_state(
            &loaded,
            &working_dir,
            &scan_result,
            EvaluationScope::Scoped(&work_unit),
        );
        let current_step = state.planned_entries.first().cloned();
        if let Some(step) = &current_step {
            validate_current(step, &loaded).map_err(|reason| SessionError::UnservableStep {
                protocol: step.protocol.clone(),
                reason,
            })?;
        }
        let last_readiness = readiness_report(&loaded.manifest.name, &state);

        Ok(Self {
            loaded,
            working_dir,
            work_unit,
            current_step,
            last_readiness,
        })
    }

    pub fn current_step(&self) -> Option<&PlannedEntry> {
        self.current_step.as_ref()
    }

    pub fn protocol(&self, name: &str) -> Option<&ProtocolDeclaration> {
        self.loaded
            .manifest
            .protocols
            .iter()
            .find(|protocol| protocol.name == name)
    }

    pub fn store(&self) -> &crate::ArtifactStore {
        &self.loaded.store
    }

    pub fn store_mut(&mut self) -> &mut crate::ArtifactStore {
        &mut self.loaded.store
    }

    pub fn readiness(&mut self) -> Result<&ReadinessReport, SessionError> {
        let state = self.scan_and_evaluate()?;
        self.last_readiness = readiness_report(&self.loaded.manifest.name, &state);
        Ok(&self.last_readiness)
    }

    pub fn next_context(&mut self) -> Result<Option<ContextInjection>, SessionError> {
        let _ = self.readiness()?;
        Ok(self.current_step.as_ref().map(|step| step.context.clone()))
    }

    pub fn advance<F>(&mut self, validate_current: F) -> Result<AdvanceReport, SessionError>
    where
        F: Fn(&PlannedEntry, &LoadedProject) -> Result<(), String>,
    {
        let advanced_step = self.current_step.clone();

        if let Some(step) = &advanced_step {
            let protocol = self
                .loaded
                .manifest
                .protocols
                .iter()
                .find(|protocol| protocol.name == step.protocol)
                .expect("current step protocol must exist in manifest");
            crate::enforce_postconditions(protocol, &self.loaded.store, step.work_unit.as_deref())
                .map_err(SessionError::Enforcement)?;
            self.loaded
                .store
                .record_execution(
                    &step.protocol,
                    step.work_unit.as_deref(),
                    step.execution_record.clone(),
                )
                .map_err(SessionError::Store)?;
        }

        let state = self.scan_and_evaluate()?;
        let next_step = state.planned_entries.first().cloned();
        if let Some(step) = &next_step {
            validate_current(step, &self.loaded).map_err(|reason| {
                SessionError::UnservableStep {
                    protocol: step.protocol.clone(),
                    reason,
                }
            })?;
        }

        self.current_step = next_step;
        self.last_readiness = readiness_report(&self.loaded.manifest.name, &state);

        Ok(AdvanceReport {
            advanced_step: advanced_step.as_ref().map(StepSummary::from),
            current_step: self.current_step.as_ref().map(StepSummary::from),
            readiness: self.last_readiness.clone(),
        })
    }

    fn scan_and_evaluate(&mut self) -> Result<ExecutionState, SessionError> {
        let scan_result = crate::scan(&self.loaded.workspace_dir, &mut self.loaded.store)
            .map_err(SessionError::Scan)?;
        Ok(evaluate_execution_state(
            &self.loaded,
            &self.working_dir,
            &scan_result,
            EvaluationScope::Scoped(&self.work_unit),
        ))
    }
}

fn readiness_report(methodology: &str, state: &ExecutionState) -> ReadinessReport {
    ReadinessReport {
        methodology: methodology.to_string(),
        scan_warnings: state.scan_findings.warnings.clone(),
        protocols: state.evaluated.json_protocols(),
    }
}

fn collect_inputs(
    protocol: &ProtocolDeclaration,
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
            if matches!(state.status, ValidationStatus::Valid) {
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
            if matches!(state.status, ValidationStatus::Valid) {
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

fn failure_entry(failure: &ArtifactFailure) -> FailureEntry {
    match failure {
        ArtifactFailure::Missing { artifact_type, .. } => FailureEntry {
            artifact_type: artifact_type.clone(),
            reason: "missing",
        },
        ArtifactFailure::Invalid { artifact_type, .. } => FailureEntry {
            artifact_type: artifact_type.clone(),
            reason: "invalid",
        },
        ArtifactFailure::Stale { artifact_type, .. } => FailureEntry {
            artifact_type: artifact_type.clone(),
            reason: "stale",
        },
        ArtifactFailure::RequiredChoiceMissing { choice, .. } => FailureEntry {
            artifact_type: choice.clone(),
            reason: "missing_choice",
        },
        ArtifactFailure::RequiredChoiceConflict { choice, .. } => FailureEntry {
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
