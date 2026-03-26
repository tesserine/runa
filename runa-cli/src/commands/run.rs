use std::collections::HashSet;
use std::fmt;
use std::path::Path;
use std::process;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use libagent::context::ContextInjectionView;
use serde::{Serialize, Serializer};
use tracing::info;

use super::CommandError;
use crate::commands::protocol_eval;
use crate::commands::step::{
    ExecutionOptions, ExecutionState, McpServerConfig, PlanEntry, PlannedEntry, StepError,
    build_plan_entries, evaluate_execution_state, execute_entry, locate_runa_mcp,
    preview_runa_mcp_command,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    AllComplete,
    QuiescentFailures,
    QuiescentBlocked,
    Interrupted,
}

impl RunOutcome {
    pub fn exit_code(self) -> i32 {
        match self {
            RunOutcome::AllComplete => 0,
            RunOutcome::QuiescentFailures => 2,
            RunOutcome::QuiescentBlocked => 3,
            RunOutcome::Interrupted => 130,
        }
    }

    fn label(self) -> &'static str {
        match self {
            RunOutcome::AllComplete => "all_complete",
            RunOutcome::QuiescentFailures => "quiescent_with_failures",
            RunOutcome::QuiescentBlocked => "quiescent_with_blocked_work",
            RunOutcome::Interrupted => "interrupted",
        }
    }
}

#[derive(Debug)]
pub enum RunError {
    Step(StepError),
    Json(serde_json::Error),
    InterruptHandler(ctrlc::Error),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Step(err) => write!(f, "{err}"),
            RunError::Json(err) => write!(f, "{err}"),
            RunError::InterruptHandler(err) => write!(f, "failed to install Ctrl-C handler: {err}"),
        }
    }
}

impl std::error::Error for RunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RunError::Step(err) => Some(err),
            RunError::Json(err) => Some(err),
            RunError::InterruptHandler(err) => Some(err),
        }
    }
}

impl From<StepError> for RunError {
    fn from(err: StepError) -> Self {
        RunError::Step(err)
    }
}

struct InterruptState {
    requested: Arc<AtomicBool>,
}

impl InterruptState {
    fn install() -> Result<Self, RunError> {
        let requested = Arc::new(AtomicBool::new(false));
        let handler_requested = Arc::clone(&requested);
        ctrlc::set_handler(move || {
            if handler_requested.swap(true, Ordering::SeqCst) {
                process::exit(130);
            }
        })
        .map_err(RunError::InterruptHandler)?;

        Ok(Self { requested })
    }

    fn requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum ProjectionKind {
    Current,
    Projected,
}

#[derive(Serialize)]
struct RunJson<'a> {
    version: u32,
    methodology: &'a str,
    scan_warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cycle: Option<Vec<String>>,
    execution_plan: Vec<RunPlanJson>,
    protocols: Vec<protocol_eval::ProtocolJson>,
}

#[derive(Serialize)]
struct RunPlanJson {
    protocol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    work_unit: Option<String>,
    trigger: String,
    projection: ProjectionKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    mcp_config: Option<McpServerConfig>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_optional_context"
    )]
    context: Option<libagent::context::ContextInjection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CandidateKey {
    protocol: String,
    work_unit: Option<String>,
}

fn serialize_optional_context<S>(
    context: &Option<libagent::context::ContextInjection>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match context {
        Some(context) => ContextInjectionView::from(context).serialize(serializer),
        None => serializer.serialize_none(),
    }
}

fn candidate_key(protocol: &str, work_unit: Option<&str>) -> CandidateKey {
    CandidateKey {
        protocol: protocol.to_string(),
        work_unit: work_unit.map(str::to_owned),
    }
}

fn resolve_agent_command(
    working_dir: &Path,
    config_override: Option<&Path>,
) -> Result<Vec<String>, RunError> {
    let config = crate::project::read_config(working_dir, config_override)
        .map_err(CommandError::from)
        .map_err(StepError::from)
        .map_err(RunError::from)?;
    config
        .agent
        .command
        .filter(|command| {
            !command.is_empty() && !command.first().is_some_and(|part| part.is_empty())
        })
        .ok_or(RunError::from(StepError::AgentCommandNotConfigured))
}

fn classify_outcome(
    evaluated: &protocol_eval::EvaluatedProtocols,
    had_failures: bool,
) -> RunOutcome {
    if had_failures {
        return RunOutcome::QuiescentFailures;
    }

    let has_blocked = evaluated.cycle.is_some()
        || !evaluated.blocked.is_empty()
        || evaluated.waiting.iter().any(|entry| {
            entry
                .unsatisfied_conditions
                .iter()
                .any(|c| c != "outputs are current")
        });

    if has_blocked {
        RunOutcome::QuiescentBlocked
    } else {
        RunOutcome::AllComplete
    }
}

fn build_run_json_plan(
    loaded: &crate::project::LoadedProject,
    working_dir: &Path,
    config_path: &Path,
    execution_state: &ExecutionState,
) -> Result<Vec<RunPlanJson>, RunError> {
    let preview_command = preview_runa_mcp_command();
    let concrete_entries: std::collections::HashMap<_, _> = build_plan_entries(
        execution_state.planned_entries.clone(),
        &preview_command,
        working_dir,
        config_path,
    )
    .into_iter()
    .map(|entry| {
        (
            candidate_key(&entry.protocol, entry.work_unit.as_deref()),
            entry,
        )
    })
    .collect();
    let protocol_map: std::collections::HashMap<&str, &libagent::ProtocolDeclaration> = loaded
        .manifest
        .protocols
        .iter()
        .map(|protocol| (protocol.name.as_str(), protocol))
        .collect();
    let topological_order = match &execution_state.evaluated.cycle {
        Some(cycle) => {
            let cycle_participants: std::collections::HashSet<&str> =
                cycle.path.iter().map(|name| name.as_str()).collect();
            loaded
                .graph
                .topological_order_excluding(&cycle_participants)
        }
        None => loaded
            .graph
            .topological_order()
            .expect("evaluated execution state must already have resolved cycle handling"),
    };
    let initial_ready: Vec<_> = execution_state
        .planned_entries
        .iter()
        .map(|entry| libagent::Candidate {
            protocol_name: entry.protocol.clone(),
            work_unit: entry.work_unit.clone(),
        })
        .collect();

    Ok(libagent::project_cascade(
        &loaded.manifest.protocols,
        &loaded.store,
        &topological_order,
        &initial_ready,
        &execution_state.scan_findings.affected_types,
    )
    .into_iter()
    .map(|entry| {
        let key = candidate_key(&entry.protocol_name, entry.work_unit.as_deref());
        let trigger = protocol_map
            .get(entry.protocol_name.as_str())
            .expect("projected protocol must exist in manifest")
            .trigger
            .to_string();

        match entry.projection {
            libagent::ProjectionClass::Current => {
                let concrete = concrete_entries
                    .get(&key)
                    .expect("current projection candidate must have concrete plan entry");
                RunPlanJson {
                    protocol: concrete.protocol.clone(),
                    work_unit: concrete.work_unit.clone(),
                    trigger: concrete.trigger.clone(),
                    projection: ProjectionKind::Current,
                    mcp_config: Some(concrete.mcp_config.clone()),
                    context: Some(concrete.context.clone()),
                }
            }
            libagent::ProjectionClass::Projected => RunPlanJson {
                protocol: entry.protocol_name,
                work_unit: entry.work_unit,
                trigger,
                projection: ProjectionKind::Projected,
                mcp_config: None,
                context: None,
            },
        }
    })
    .collect())
}

enum ReconcileOutcome {
    Succeeded {
        scan_result: libagent::ScanResult,
        execution_entry: Box<PlanEntry>,
    },
    PostconditionFailure {
        scan_result: libagent::ScanResult,
    },
}

fn execute_and_reconcile(
    working_dir: &Path,
    loaded: &mut crate::project::LoadedProject,
    agent_command: &[String],
    config_path: &Path,
    mcp_command: &str,
    next_entry: PlannedEntry,
) -> Result<ReconcileOutcome, StepError> {
    let execution_entry =
        build_plan_entries(vec![next_entry], mcp_command, working_dir, config_path)
            .into_iter()
            .next()
            .expect("single planned entry must produce one execution entry");

    execute_entry(
        working_dir,
        agent_command,
        &execution_entry,
        ExecutionOptions {
            isolate_process_group: true,
        },
    )?;

    let scan_result =
        libagent::scan(&loaded.workspace_dir, &mut loaded.store).map_err(|source| {
            StepError::PostExecutionScan {
                protocol: execution_entry.protocol.clone(),
                work_unit: execution_entry.work_unit.clone(),
                source,
            }
        })?;

    let protocol = loaded
        .manifest
        .protocols
        .iter()
        .find(|protocol| protocol.name == execution_entry.protocol)
        .expect("planned protocol must exist in manifest");
    if libagent::enforce_postconditions(
        protocol,
        &loaded.store,
        execution_entry.work_unit.as_deref(),
    )
    .is_err()
    {
        return Ok(ReconcileOutcome::PostconditionFailure { scan_result });
    }

    Ok(ReconcileOutcome::Succeeded {
        scan_result,
        execution_entry: Box::new(execution_entry),
    })
}

fn refresh_state_after_scan(
    loaded: &crate::project::LoadedProject,
    working_dir: &Path,
    exhausted: &mut HashSet<CandidateKey>,
    scan_result: &libagent::ScanResult,
) -> ExecutionState {
    exhausted.retain(|candidate| {
        let protocol = loaded
            .manifest
            .protocols
            .iter()
            .find(|protocol| protocol.name == candidate.protocol)
            .expect("planned protocol must exist in manifest");
        !libagent::protocol_relevant_inputs_changed(
            protocol,
            candidate.work_unit.as_deref(),
            scan_result,
        )
    });

    evaluate_execution_state(loaded, working_dir, scan_result)
}

pub fn run(
    working_dir: &Path,
    config_override: Option<&Path>,
    dry_run: bool,
    json_output: bool,
) -> Result<RunOutcome, RunError> {
    if !dry_run && json_output {
        return Err(RunError::from(StepError::JsonRequiresDryRun));
    }

    let (mut loaded, scan_result) = super::load_and_scan(working_dir, config_override)
        .map_err(StepError::from)
        .map_err(RunError::from)?;
    let initial_state = evaluate_execution_state(&loaded, working_dir, &scan_result);
    let config_path = crate::project::resolve_config(working_dir, config_override)
        .map_err(CommandError::from)
        .map_err(StepError::from)
        .map_err(RunError::from)?;

    if dry_run {
        let execution_plan =
            build_run_json_plan(&loaded, working_dir, &config_path, &initial_state)?;

        if json_output {
            let payload = RunJson {
                version: 1,
                methodology: &loaded.manifest.name,
                scan_warnings: initial_state.scan_findings.warnings.clone(),
                cycle: initial_state
                    .evaluated
                    .cycle
                    .as_ref()
                    .map(|cycle| cycle.path.clone()),
                execution_plan,
                protocols: initial_state.evaluated.json_protocols(),
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&payload).map_err(RunError::Json)?
            );
        } else {
            println!("Methodology: {}", loaded.manifest.name);
            println!();
            if execution_plan.is_empty() {
                println!("Execution plan: none");
            } else {
                println!("Execution plan:");
                for (index, entry) in execution_plan.iter().enumerate() {
                    match &entry.work_unit {
                        Some(work_unit) => println!(
                            "  {}. {} (work_unit={work_unit}) [{}]",
                            index + 1,
                            entry.protocol,
                            match entry.projection {
                                ProjectionKind::Current => "current",
                                ProjectionKind::Projected => "projected",
                            }
                        ),
                        None => println!(
                            "  {}. {} [{}]",
                            index + 1,
                            entry.protocol,
                            match entry.projection {
                                ProjectionKind::Current => "current",
                                ProjectionKind::Projected => "projected",
                            }
                        ),
                    }
                }
            }
            println!();
            protocol_eval::print_group("READY", &initial_state.evaluated.ready);
            println!();
            protocol_eval::print_group("BLOCKED", &initial_state.evaluated.blocked);
            println!();
            protocol_eval::print_group("WAITING", &initial_state.evaluated.waiting);
        }

        return Ok(classify_outcome(&initial_state.evaluated, false));
    }

    let mut state = initial_state;
    if state.planned_entries.is_empty() {
        let outcome = classify_outcome(&state.evaluated, false);
        println!("Run outcome: {}", outcome.label());
        return Ok(outcome);
    }

    let agent_command = resolve_agent_command(working_dir, config_override)?;
    let mcp_command = locate_runa_mcp()
        .map_err(RunError::from)?
        .to_string_lossy()
        .into_owned();
    let interrupts = InterruptState::install()?;
    let mut exhausted = HashSet::new();
    let mut failed = HashSet::new();

    loop {
        let next_entry = state.planned_entries.clone().into_iter().find(|entry| {
            let key = candidate_key(&entry.protocol, entry.work_unit.as_deref());
            !exhausted.contains(&key) && !failed.contains(&key)
        });
        let Some(next_entry) = next_entry else {
            let outcome = classify_outcome(&state.evaluated, !failed.is_empty());
            println!("Run outcome: {}", outcome.label());
            return Ok(outcome);
        };
        if interrupts.requested() {
            info!(
                operation = "run",
                outcome = "interrupted",
                "stopping after current cycle"
            );
            println!("Run outcome: {}", RunOutcome::Interrupted.label());
            return Ok(RunOutcome::Interrupted);
        }

        let key = candidate_key(&next_entry.protocol, next_entry.work_unit.as_deref());
        match execute_and_reconcile(
            working_dir,
            &mut loaded,
            &agent_command,
            &config_path,
            &mcp_command,
            next_entry,
        ) {
            Ok(ReconcileOutcome::Succeeded {
                scan_result,
                execution_entry,
            }) => {
                exhausted.insert(key);
                state =
                    refresh_state_after_scan(&loaded, working_dir, &mut exhausted, &scan_result);
                println!(
                    "Executed: {}",
                    match &execution_entry.work_unit {
                        Some(work_unit) =>
                            format!("{} (work_unit={work_unit})", execution_entry.protocol),
                        None => execution_entry.protocol,
                    }
                );
            }
            Ok(ReconcileOutcome::PostconditionFailure { scan_result }) => {
                failed.insert(key);
                state =
                    refresh_state_after_scan(&loaded, working_dir, &mut exhausted, &scan_result);
            }
            Err(StepError::AgentCommandFailed {
                protocol,
                work_unit,
                ..
            }) => {
                failed.insert(key);
                let scan_result = libagent::scan(&loaded.workspace_dir, &mut loaded.store)
                    .map_err(|source| {
                        RunError::from(StepError::PostExecutionScan {
                            protocol,
                            work_unit,
                            source,
                        })
                    })?;
                state =
                    refresh_state_after_scan(&loaded, working_dir, &mut exhausted, &scan_result);
            }
            Err(err) => return Err(RunError::from(err)),
        }
    }
}
