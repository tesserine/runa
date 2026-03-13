use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

use libagent::{TriggerContext, TriggerResult, enforce_preconditions, evaluate_trigger};
use serde::Serialize;

use crate::project::{self, ProjectError};

#[derive(Debug)]
pub enum StatusError {
    Project(ProjectError),
    Scan(libagent::ScanError),
}

impl fmt::Display for StatusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StatusError::Project(err) => write!(f, "{err}"),
            StatusError::Scan(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for StatusError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StatusError::Project(err) => Some(err),
            StatusError::Scan(err) => Some(err),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum SkillStatus {
    Ready,
    Blocked,
    Waiting,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum TriggerState {
    Satisfied,
    NotSatisfied,
}

struct SkillEntry {
    name: String,
    status: SkillStatus,
    trigger: TriggerState,
    inputs: Vec<InputEntry>,
    precondition_failures: Vec<FailureEntry>,
    unsatisfied_conditions: Vec<String>,
}

#[derive(Serialize)]
struct StatusJson<'a> {
    version: u32,
    methodology: &'a str,
    scan_warnings: Vec<String>,
    skills: Vec<SkillJson>,
}

#[derive(Serialize)]
struct SkillJson {
    name: String,
    status: SkillStatus,
    trigger: TriggerState,
    #[serde(skip_serializing_if = "Option::is_none")]
    inputs: Option<Vec<InputJson>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    precondition_failures: Option<Vec<FailureJson>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unsatisfied_conditions: Option<Vec<String>>,
}

#[derive(Serialize)]
struct InputJson {
    artifact_type: String,
    instance_id: String,
    path: String,
    relationship: &'static str,
}

#[derive(Serialize)]
struct FailureJson {
    artifact_type: String,
    reason: &'static str,
}

struct InputEntry {
    artifact_type: String,
    instance_id: String,
    path: String,
    relationship: &'static str,
}

struct FailureEntry {
    artifact_type: String,
    reason: &'static str,
}

struct ScanFindings {
    affected_types: HashSet<String>,
    warnings: Vec<String>,
}

pub fn run(
    working_dir: &Path,
    config_override: Option<&Path>,
    json_output: bool,
) -> Result<(), StatusError> {
    let mut loaded = project::load(working_dir, config_override).map_err(StatusError::Project)?;
    let scan_result =
        libagent::scan(&loaded.workspace_dir, &mut loaded.store).map_err(StatusError::Scan)?;
    let scan_findings = collect_scan_findings(&scan_result, &loaded.workspace_dir);

    let skill_order = match loaded.graph.topological_order() {
        Ok(order) => order,
        Err(cycle) => {
            eprintln!("warning: {cycle}");
            loaded
                .manifest
                .skills
                .iter()
                .map(|skill| skill.name.as_str())
                .collect()
        }
    };

    let skill_map: HashMap<&str, &libagent::SkillDeclaration> = loaded
        .manifest
        .skills
        .iter()
        .map(|skill| (skill.name.as_str(), skill))
        .collect();

    let timestamps = HashMap::new();
    let signals = HashSet::new();
    let context = TriggerContext {
        store: &loaded.store,
        activation_timestamps: &timestamps,
        active_signals: &signals,
    };

    let mut ready = Vec::new();
    let mut blocked = Vec::new();
    let mut waiting = Vec::new();

    for name in skill_order {
        let Some(skill) = skill_map.get(name) else {
            continue;
        };

        let trigger_state = match evaluate_trigger(&skill.trigger, &context, &skill.name) {
            TriggerResult::Satisfied => TriggerState::Satisfied,
            TriggerResult::NotSatisfied(_) => TriggerState::NotSatisfied,
        };

        let entry = if let Some(artifact_type) = skill.requires.iter().find(|artifact_type| {
            scan_findings
                .affected_types
                .contains(artifact_type.as_str())
        }) {
            SkillEntry {
                name: skill.name.clone(),
                status: SkillStatus::Blocked,
                trigger: trigger_state,
                inputs: Vec::new(),
                precondition_failures: vec![FailureEntry {
                    artifact_type: artifact_type.clone(),
                    reason: "scan_incomplete",
                }],
                unsatisfied_conditions: Vec::new(),
            }
        } else {
            match trigger_state {
                TriggerState::Satisfied => match enforce_preconditions(skill, &loaded.store) {
                    Ok(()) => SkillEntry {
                        name: skill.name.clone(),
                        status: SkillStatus::Ready,
                        trigger: TriggerState::Satisfied,
                        inputs: collect_inputs(
                            skill,
                            &loaded.store,
                            working_dir,
                            &scan_findings.affected_types,
                        ),
                        precondition_failures: Vec::new(),
                        unsatisfied_conditions: Vec::new(),
                    },
                    Err(err) => SkillEntry {
                        name: skill.name.clone(),
                        status: SkillStatus::Blocked,
                        trigger: TriggerState::Satisfied,
                        inputs: Vec::new(),
                        precondition_failures: err.failures.iter().map(failure_entry).collect(),
                        unsatisfied_conditions: Vec::new(),
                    },
                },
                TriggerState::NotSatisfied => SkillEntry {
                    name: skill.name.clone(),
                    status: SkillStatus::Waiting,
                    trigger: TriggerState::NotSatisfied,
                    inputs: Vec::new(),
                    precondition_failures: Vec::new(),
                    unsatisfied_conditions: collect_unsatisfied_conditions(
                        &skill.trigger,
                        &context,
                        &skill.name,
                    ),
                },
            }
        };

        match entry.status {
            SkillStatus::Ready => ready.push(entry),
            SkillStatus::Blocked => blocked.push(entry),
            SkillStatus::Waiting => waiting.push(entry),
        }
    }

    if json_output {
        let payload = StatusJson {
            version: 1,
            methodology: &loaded.manifest.name,
            scan_warnings: scan_findings.warnings.clone(),
            skills: ready
                .into_iter()
                .chain(blocked)
                .chain(waiting)
                .map(skill_json)
                .collect(),
        };
        println!("{}", serde_json::to_string_pretty(&payload).unwrap());
    } else {
        println!("Methodology: {}", loaded.manifest.name);
        if !scan_findings.warnings.is_empty() {
            println!();
            println!("Scan warnings:");
            for warning in &scan_findings.warnings {
                println!("  - {warning}");
            }
        }
        println!();
        print_group("READY", &ready);
        println!();
        print_group("BLOCKED", &blocked);
        println!();
        print_group("WAITING", &waiting);
    }

    Ok(())
}

fn collect_inputs(
    skill: &libagent::SkillDeclaration,
    store: &libagent::ArtifactStore,
    working_dir: &Path,
    affected_types: &HashSet<String>,
) -> Vec<InputEntry> {
    let mut inputs = Vec::new();

    for artifact_type in &skill.requires {
        if affected_types.contains(artifact_type) {
            continue;
        }
        for (instance_id, state) in store.instances_of(artifact_type) {
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

    for artifact_type in &skill.accepts {
        if affected_types.contains(artifact_type) {
            continue;
        }
        for (instance_id, state) in store.instances_of(artifact_type) {
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

fn collect_scan_findings(scan_result: &libagent::ScanResult, workspace_dir: &Path) -> ScanFindings {
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

fn artifact_type_from_path(path: &Path, workspace_dir: &Path) -> Option<String> {
    let relative = path.strip_prefix(workspace_dir).ok()?;
    let mut components = relative.components();
    let first = components.next()?;
    Some(PathBuf::from(first.as_os_str()).display().to_string())
}

fn print_group(label: &str, entries: &[SkillEntry]) {
    println!("{label}:");
    if entries.is_empty() {
        println!("  (none)");
        return;
    }

    for entry in entries {
        println!("  {}", entry.name);
        match entry.status {
            SkillStatus::Ready => {
                for input in &entry.inputs {
                    println!(
                        "    - {}/{} ({})",
                        input.artifact_type, input.instance_id, input.relationship
                    );
                }
            }
            SkillStatus::Blocked => {
                for failure in &entry.precondition_failures {
                    println!("    - {} ({})", failure.artifact_type, failure.reason);
                }
            }
            SkillStatus::Waiting => {
                for condition in &entry.unsatisfied_conditions {
                    println!("    - {condition}");
                }
            }
        }
    }
}

fn collect_unsatisfied_conditions(
    condition: &libagent::TriggerCondition,
    context: &TriggerContext<'_>,
    skill_name: &str,
) -> Vec<String> {
    if matches!(
        evaluate_trigger(condition, context, skill_name),
        TriggerResult::Satisfied
    ) {
        return Vec::new();
    }

    match condition {
        libagent::TriggerCondition::AllOf { conditions } => conditions
            .iter()
            .flat_map(|child| collect_unsatisfied_conditions(child, context, skill_name))
            .collect(),
        libagent::TriggerCondition::AnyOf { conditions } => conditions
            .iter()
            .flat_map(|child| collect_unsatisfied_conditions(child, context, skill_name))
            .collect(),
        _ => vec![condition.to_string()],
    }
}

fn skill_json(entry: SkillEntry) -> SkillJson {
    SkillJson {
        name: entry.name,
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
