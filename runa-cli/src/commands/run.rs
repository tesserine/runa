use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::path::Path;
use std::process;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use libagent::{CandidateKey, context::ContextInjectionView};
use serde::{Serialize, Serializer};
use tracing::info;

use super::CommandError;
use crate::commands::step::{
    ExecutionOptions, ExecutionState, McpServerConfig, PlanEntry, PlannedEntry, StepError,
    build_plan_entries, evaluate_execution_state, execute_entry, locate_runa_mcp,
    preview_runa_mcp_command,
};
use crate::commands::{entry, protocol_eval};
use crate::exit_codes::ExitCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    AllComplete,
    NothingReady,
    QuiescentFailures,
    QuiescentBlocked,
    Interrupted,
}

impl RunOutcome {
    pub fn exit_code(self) -> i32 {
        match self {
            RunOutcome::Interrupted => 130,
            _ => self.as_exit_code().code(),
        }
    }

    pub const fn as_exit_code(self) -> ExitCode {
        match self {
            RunOutcome::AllComplete => ExitCode::Success,
            RunOutcome::NothingReady => ExitCode::NothingReady,
            RunOutcome::QuiescentFailures => ExitCode::WorkFailed,
            RunOutcome::QuiescentBlocked => ExitCode::Blocked,
            RunOutcome::Interrupted => ExitCode::Success,
        }
    }

    fn label(self) -> &'static str {
        match self {
            RunOutcome::AllComplete => "success",
            RunOutcome::NothingReady => "nothing_ready",
            RunOutcome::QuiescentFailures => "work_failed",
            RunOutcome::QuiescentBlocked => "blocked",
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

impl RunError {
    pub(crate) fn exit_code(&self) -> ExitCode {
        match self {
            RunError::Step(err) => err.exit_code(),
            RunError::Json(_) | RunError::InterruptHandler(_) => ExitCode::InfrastructureFailure,
        }
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
struct EntryJson {
    reference: String,
    ticket_number: u64,
    acquisition_protocol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolved_work_unit: Option<String>,
}

#[derive(Serialize)]
struct RunEntryJson<'a> {
    version: u32,
    methodology: &'a str,
    entry: EntryJson,
    scan_warnings: Vec<String>,
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
    CandidateKey::new(protocol, work_unit)
}

fn resolve_agent_command(
    working_dir: &Path,
    config_override: Option<&Path>,
    cli_override_present: bool,
    cli_override: &[String],
) -> Result<Vec<String>, RunError> {
    if cli_override_present {
        return if is_usable_agent_command(cli_override) {
            Ok(cli_override.to_vec())
        } else {
            Err(RunError::from(StepError::AgentCommandNotConfigured))
        };
    }

    let config = crate::project::read_config(working_dir, config_override)
        .map_err(CommandError::from)
        .map_err(StepError::from)
        .map_err(RunError::from)?;
    config
        .agent
        .command
        .filter(|command| is_usable_agent_command(command))
        .ok_or(RunError::from(StepError::AgentCommandNotConfigured))
}

fn is_usable_agent_command(command: &[String]) -> bool {
    !command.is_empty() && !command.first().is_some_and(|part| part.is_empty())
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
        || evaluated
            .waiting
            .iter()
            .any(|entry| entry.waiting_reason != Some(libagent::WaitingReason::OutputsCurrent));

    if has_blocked {
        RunOutcome::QuiescentBlocked
    } else {
        RunOutcome::AllComplete
    }
}

fn classify_live_outcome(
    evaluated: &protocol_eval::EvaluatedProtocols,
    had_failures: bool,
    executed_any: bool,
) -> RunOutcome {
    match classify_outcome(evaluated, had_failures) {
        RunOutcome::AllComplete if !executed_any => RunOutcome::NothingReady,
        outcome => outcome,
    }
}

fn build_run_json_plan(
    loaded: &crate::project::LoadedProject,
    working_dir: &Path,
    config_path: &Path,
    execution_state: &ExecutionState,
    scope: libagent::EvaluationScope<'_>,
) -> Result<Vec<RunPlanJson>, RunError> {
    let preview_command = preview_runa_mcp_command();
    let runtime_env = crate::commands::step::resolved_runtime_env(working_dir, &loaded.config);
    let concrete_entries: std::collections::HashMap<_, _> = build_plan_entries(
        execution_state.planned_entries.clone(),
        &preview_command,
        working_dir,
        config_path,
        &runtime_env,
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
    let topological_order: Vec<&str> = execution_state
        .evaluated
        .topology
        .execution_order
        .iter()
        .map(String::as_str)
        .collect();
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
        scope,
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
    runtime_env: &BTreeMap<String, String>,
) -> Result<ReconcileOutcome, StepError> {
    let transcript_settings = libagent::transcript::resolve_transcript_settings_with_forge(
        working_dir,
        &loaded.config.transcript,
        &loaded.config.forge,
    );
    let execution_entry = build_plan_entries(
        vec![next_entry],
        mcp_command,
        working_dir,
        config_path,
        runtime_env,
    )
    .into_iter()
    .next()
    .expect("single planned entry must produce one execution entry");

    execute_entry(
        working_dir,
        agent_command,
        &execution_entry,
        ExecutionOptions {
            isolate_process_group: true,
            extra_env: runtime_env.clone(),
            transcript_settings: Some(transcript_settings),
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

    loaded
        .store
        .record_execution(
            &execution_entry.protocol,
            execution_entry.work_unit.as_deref(),
            execution_entry.execution_record.clone(),
        )
        .map_err(|source| StepError::PostExecutionRecord {
            protocol: execution_entry.protocol.clone(),
            work_unit: execution_entry.work_unit.clone(),
            source,
        })?;

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
    scope: libagent::EvaluationScope<'_>,
) -> ExecutionState {
    libagent::refresh_exhausted_candidates_after_scan(
        &loaded.manifest.protocols,
        exhausted,
        scan_result,
    );

    evaluate_execution_state(loaded, working_dir, scan_result, scope)
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    working_dir: &Path,
    config_override: Option<&Path>,
    dry_run: bool,
    json_output: bool,
    work_unit: Option<&str>,
    ticket: Option<&str>,
    cli_agent_command_present: bool,
    cli_agent_command_argv: &[String],
) -> Result<RunOutcome, RunError> {
    if !dry_run && json_output {
        return Err(RunError::from(StepError::JsonRequiresDryRun));
    }

    let (mut loaded, scan_result) = super::load_and_scan(working_dir, config_override)
        .map_err(StepError::from)
        .map_err(RunError::from)?;

    if let Some(raw) = ticket {
        let (ticket_ref, identity) =
            entry::resolve_reference(&loaded, raw).map_err(RunError::from)?;

        // Re-entry: the work-unit already exists — behave as `--work-unit <id>`.
        if let Some(work_unit) =
            entry::resolve_existing(&loaded, &identity, &ticket_ref).map_err(RunError::from)?
        {
            return run_with_scope(
                working_dir,
                config_override,
                dry_run,
                json_output,
                loaded,
                scan_result,
                Some(work_unit),
                false,
                cli_agent_command_present,
                cli_agent_command_argv,
            );
        }

        // Cold: project the entry cascade, or execute acquisition then cascade.
        if dry_run {
            return run_ticket_dry_run(
                &loaded,
                working_dir,
                config_override,
                &scan_result,
                &ticket_ref,
                json_output,
            );
        }
        // Entry substitutes only the trigger; the acquisition's preconditions
        // and scan trust still gate. Block before launching the agent when unmet.
        let acquisition = entry::acquisition_surface(&loaded).map_err(RunError::from)?;
        if let Some(reason) = entry::acquisition_block_reason(&loaded, &acquisition, &scan_result) {
            println!("Run outcome: {}", RunOutcome::QuiescentBlocked.label());
            println!("{reason}");
            return Ok(RunOutcome::QuiescentBlocked);
        }
        let agent_command = resolve_agent_command(
            working_dir,
            config_override,
            cli_agent_command_present,
            cli_agent_command_argv,
        )?;
        let (work_unit, scan_result) = acquire_ticket(
            working_dir,
            config_override,
            &mut loaded,
            &scan_result,
            &ticket_ref,
            &identity,
            &agent_command,
        )?;
        // Acquisition already executed; carry that into the downstream outcome
        // so a quiescent post-acquisition scope reports success, not nothing-ready.
        return run_with_scope(
            working_dir,
            config_override,
            dry_run,
            json_output,
            loaded,
            scan_result,
            Some(work_unit),
            true,
            cli_agent_command_present,
            cli_agent_command_argv,
        );
    }

    run_with_scope(
        working_dir,
        config_override,
        dry_run,
        json_output,
        loaded,
        scan_result,
        work_unit.map(str::to_owned),
        false,
        cli_agent_command_present,
        cli_agent_command_argv,
    )
}

/// Execute the cascade for a fixed scope (a delegated work-unit or unscoped).
#[allow(clippy::too_many_arguments)]
fn run_with_scope(
    working_dir: &Path,
    config_override: Option<&Path>,
    dry_run: bool,
    json_output: bool,
    mut loaded: crate::project::LoadedProject,
    scan_result: libagent::ScanResult,
    work_unit: Option<String>,
    prior_execution: bool,
    cli_agent_command_present: bool,
    cli_agent_command_argv: &[String],
) -> Result<RunOutcome, RunError> {
    super::validate_scoped_work_unit(&loaded, work_unit.as_deref())
        .map_err(StepError::from)
        .map_err(RunError::from)?;
    let scope = match work_unit.as_deref() {
        Some(work_unit) => libagent::EvaluationScope::Scoped(work_unit),
        None => libagent::EvaluationScope::Unscoped,
    };
    let initial_state = evaluate_execution_state(&loaded, working_dir, &scan_result, scope);
    let config_path = crate::project::resolve_config(working_dir, config_override)
        .map_err(CommandError::from)
        .map_err(StepError::from)
        .map_err(RunError::from)?;

    if dry_run {
        let execution_plan =
            build_run_json_plan(&loaded, working_dir, &config_path, &initial_state, scope)?;

        if json_output {
            let payload = RunJson {
                version: 2,
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

    let agent_command = resolve_agent_command(
        working_dir,
        config_override,
        cli_agent_command_present,
        cli_agent_command_argv,
    )?;
    let mut state = initial_state;
    if state.planned_entries.is_empty() {
        let outcome = classify_live_outcome(&state.evaluated, false, prior_execution);
        println!("Run outcome: {}", outcome.label());
        return Ok(outcome);
    }

    let mcp_command = locate_runa_mcp()
        .map_err(RunError::from)?
        .to_string_lossy()
        .into_owned();
    let interrupts = InterruptState::install()?;
    let runtime_env = crate::commands::step::resolved_runtime_env(working_dir, &loaded.config);
    let mut exhausted = HashSet::new();
    let mut failed = HashSet::new();
    let mut executed_any = prior_execution;

    loop {
        let next_entry = state.planned_entries.clone().into_iter().find(|entry| {
            let key = candidate_key(&entry.protocol, entry.work_unit.as_deref());
            !exhausted.contains(&key) && !failed.contains(&key)
        });
        let Some(next_entry) = next_entry else {
            let outcome = classify_live_outcome(&state.evaluated, !failed.is_empty(), executed_any);
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
            &runtime_env,
        ) {
            Ok(ReconcileOutcome::Succeeded {
                scan_result,
                execution_entry,
            }) => {
                executed_any = true;
                exhausted.insert(key);
                state = refresh_state_after_scan(
                    &loaded,
                    working_dir,
                    &mut exhausted,
                    &scan_result,
                    scope,
                );
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
                state = refresh_state_after_scan(
                    &loaded,
                    working_dir,
                    &mut exhausted,
                    &scan_result,
                    scope,
                );
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
                state = refresh_state_after_scan(
                    &loaded,
                    working_dir,
                    &mut exhausted,
                    &scan_result,
                    scope,
                );
            }
            Err(err) => return Err(RunError::from(err)),
        }
    }
}

/// Project the cold-start ticket entry cascade without executing any agent.
///
/// The acquisition step is `current`; the work-unit it would produce seeds the
/// projection so `take` appears next on the acquired work-unit.
fn run_ticket_dry_run(
    loaded: &crate::project::LoadedProject,
    working_dir: &Path,
    config_override: Option<&Path>,
    scan_result: &libagent::ScanResult,
    ticket_ref: &libagent::TicketRef,
    json_output: bool,
) -> Result<RunOutcome, RunError> {
    let acquisition = entry::acquisition_surface(loaded).map_err(RunError::from)?;
    // Entry substitutes only the trigger; the acquisition's preconditions and
    // scan trust still gate. When unmet the cascade projects nothing and the
    // outcome is blocked.
    let block_reason = entry::acquisition_block_reason(loaded, &acquisition, scan_result);
    let scan_findings = libagent::collect_scan_findings(scan_result, &loaded.workspace_dir);
    let config_path = crate::project::resolve_config(working_dir, config_override)
        .map_err(CommandError::from)
        .map_err(StepError::from)
        .map_err(RunError::from)?;
    let runtime_env = entry::entry_runtime_env(working_dir, &loaded.config, ticket_ref);
    let preview_command = preview_runa_mcp_command();

    let entry_planned =
        entry::acquisition_planned_entry(loaded, &acquisition, ticket_ref, &scan_findings);
    let concrete = build_plan_entries(
        vec![entry_planned],
        &preview_command,
        working_dir,
        &config_path,
        &runtime_env,
    )
    .into_iter()
    .next()
    .expect("single planned entry must produce one execution entry");

    let graph = libagent::DependencyGraph::build(&loaded.manifest.protocols).ok();
    let fallback: Vec<&str> = loaded
        .manifest
        .protocols
        .iter()
        .map(|protocol| protocol.name.as_str())
        .collect();
    let topological_order: Vec<&str> = match &graph {
        Some(graph) => graph.topological_order_excluding(&HashSet::new()),
        None => fallback,
    };
    let promised_scope = entry::promised_scope_token(ticket_ref);
    let projected = libagent::project_entry_cascade(
        &loaded.manifest.protocols,
        &loaded.store,
        &topological_order,
        &libagent::Candidate {
            protocol_name: acquisition.name.clone(),
            work_unit: None,
        },
        &promised_scope,
        &scan_findings.affected_types,
    );

    let protocol_map: std::collections::HashMap<&str, &libagent::ProtocolDeclaration> = loaded
        .manifest
        .protocols
        .iter()
        .map(|protocol| (protocol.name.as_str(), protocol))
        .collect();
    let execution_plan: Vec<RunPlanJson> = projected
        .into_iter()
        .map(|candidate| match candidate.projection {
            libagent::ProjectionClass::Current => RunPlanJson {
                protocol: concrete.protocol.clone(),
                work_unit: concrete.work_unit.clone(),
                trigger: concrete.trigger.clone(),
                projection: ProjectionKind::Current,
                mcp_config: Some(concrete.mcp_config.clone()),
                context: Some(concrete.context.clone()),
            },
            libagent::ProjectionClass::Projected => {
                let trigger = protocol_map
                    .get(candidate.protocol_name.as_str())
                    .expect("projected protocol must exist in manifest")
                    .trigger
                    .to_string();
                RunPlanJson {
                    protocol: candidate.protocol_name,
                    work_unit: candidate.work_unit,
                    trigger,
                    projection: ProjectionKind::Projected,
                    mcp_config: None,
                    context: None,
                }
            }
        })
        .collect();

    let unscoped_state = evaluate_execution_state(
        loaded,
        working_dir,
        scan_result,
        libagent::EvaluationScope::Unscoped,
    );

    if json_output {
        let payload = RunEntryJson {
            version: 3,
            methodology: &loaded.manifest.name,
            entry: EntryJson {
                reference: ticket_ref.display.clone(),
                ticket_number: ticket_ref.number,
                acquisition_protocol: acquisition.name.clone(),
                resolved_work_unit: None,
            },
            scan_warnings: scan_findings.warnings.clone(),
            execution_plan,
            protocols: unscoped_state.evaluated.json_protocols(),
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).map_err(RunError::Json)?
        );
    } else {
        println!("Methodology: {}", loaded.manifest.name);
        println!(
            "Entry: ticket {} (acquisition: {})",
            ticket_ref.display, acquisition.name
        );
        println!();
        if let Some(reason) = &block_reason {
            println!("Execution plan: none (acquisition blocked)");
            println!();
            println!("{reason}");
        } else {
            println!("Execution plan:");
            for (index, plan_entry) in execution_plan.iter().enumerate() {
                let projection = match plan_entry.projection {
                    ProjectionKind::Current => "current, entry",
                    ProjectionKind::Projected => "projected",
                };
                match &plan_entry.work_unit {
                    Some(work_unit) => println!(
                        "  {}. {} (work_unit={work_unit}) [{projection}]",
                        index + 1,
                        plan_entry.protocol
                    ),
                    None => println!("  {}. {} [{projection}]", index + 1, plan_entry.protocol),
                }
            }
        }
    }

    if block_reason.is_some() {
        Ok(RunOutcome::QuiescentBlocked)
    } else {
        Ok(RunOutcome::AllComplete)
    }
}

/// Execute the cold-start acquisition step and resolve the materialized
/// work-unit. Returns the bound work-unit id and the post-acquisition scan.
fn acquire_ticket(
    working_dir: &Path,
    config_override: Option<&Path>,
    loaded: &mut crate::project::LoadedProject,
    scan_result: &libagent::ScanResult,
    ticket_ref: &libagent::TicketRef,
    identity: &libagent::ResolvedForgeIdentity,
    agent_command: &[String],
) -> Result<(String, libagent::ScanResult), RunError> {
    let acquisition = entry::acquisition_surface(loaded).map_err(RunError::from)?;
    let scan_findings = libagent::collect_scan_findings(scan_result, &loaded.workspace_dir);
    let entry_planned =
        entry::acquisition_planned_entry(loaded, &acquisition, ticket_ref, &scan_findings);
    let config_path = crate::project::resolve_config(working_dir, config_override)
        .map_err(CommandError::from)
        .map_err(StepError::from)
        .map_err(RunError::from)?;
    let mcp_command = locate_runa_mcp()
        .map_err(RunError::from)?
        .to_string_lossy()
        .into_owned();
    let runtime_env = entry::entry_runtime_env(working_dir, &loaded.config, ticket_ref);

    let post_scan = match execute_and_reconcile(
        working_dir,
        loaded,
        agent_command,
        &config_path,
        &mcp_command,
        entry_planned,
        &runtime_env,
    )? {
        ReconcileOutcome::Succeeded { scan_result, .. } => scan_result,
        ReconcileOutcome::PostconditionFailure { .. } => {
            return Err(RunError::from(StepError::TicketReference(
                libagent::EntryError::Unresolved {
                    reference: ticket_ref.display.clone(),
                },
            )));
        }
    };

    println!(
        "Executed: {} (entry from ticket {})",
        acquisition.name, ticket_ref.display
    );
    // The acquisition record is persisted; it only stands if the promise binds.
    // When acquisition satisfied its contract but did not materialize a work-unit
    // for this ticket, clear the record so no metadata claims the step completed.
    match entry::resolve_existing(loaded, identity, ticket_ref) {
        Ok(Some(work_unit)) => Ok((work_unit, post_scan)),
        Ok(None) => {
            clear_acquisition_record(loaded, &acquisition.name)?;
            Err(RunError::from(StepError::TicketReference(
                libagent::EntryError::Unresolved {
                    reference: ticket_ref.display.clone(),
                },
            )))
        }
        Err(error) => {
            clear_acquisition_record(loaded, &acquisition.name)?;
            Err(RunError::from(error))
        }
    }
}

fn clear_acquisition_record(
    loaded: &mut crate::project::LoadedProject,
    acquisition: &str,
) -> Result<(), RunError> {
    loaded
        .store
        .clear_execution_record(acquisition, None)
        .map_err(|source| {
            RunError::from(StepError::PostExecutionRecord {
                protocol: acquisition.to_string(),
                work_unit: None,
                source,
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::protocol_eval::{
        EvaluatedProtocols, FailureEntry, InputEntry, ProtocolEntry, ProtocolStatus, TriggerState,
    };

    fn waiting_entry(
        unsatisfied_conditions: &[&str],
        waiting_reason: libagent::WaitingReason,
    ) -> ProtocolEntry {
        ProtocolEntry {
            name: "publish".into(),
            work_unit: None,
            status: ProtocolStatus::Waiting,
            trigger: TriggerState::Satisfied,
            inputs: Vec::<InputEntry>::new(),
            precondition_failures: Vec::<FailureEntry>::new(),
            unsatisfied_conditions: unsatisfied_conditions
                .iter()
                .map(|s| s.to_string())
                .collect(),
            waiting_reason: Some(waiting_reason),
        }
    }

    fn evaluated_with_waiting(waiting: ProtocolEntry) -> EvaluatedProtocols {
        EvaluatedProtocols {
            topology: libagent::EvaluationTopology {
                status_order: Vec::new(),
                execution_order: Vec::new(),
                cycle: None,
            },
            cycle: None,
            ready: Vec::new(),
            blocked: Vec::new(),
            waiting: vec![waiting],
        }
    }

    #[test]
    fn classify_outcome_uses_waiting_reason_not_display_text_for_current_outputs() {
        let evaluated = evaluated_with_waiting(waiting_entry(
            &["fresh outputs already exist"],
            libagent::WaitingReason::OutputsCurrent,
        ));

        assert_eq!(classify_outcome(&evaluated, false), RunOutcome::AllComplete);
    }

    #[test]
    fn classify_outcome_still_blocks_when_waiting_for_non_quiescent_reason() {
        let evaluated = evaluated_with_waiting(waiting_entry(
            &["constraints missing"],
            libagent::WaitingReason::TriggerUnsatisfied,
        ));

        assert_eq!(
            classify_outcome(&evaluated, false),
            RunOutcome::QuiescentBlocked
        );
    }

    #[test]
    fn usable_agent_command_requires_non_empty_argv() {
        assert!(!is_usable_agent_command(&[]));
        assert!(!is_usable_agent_command(&[String::new()]));
        assert!(is_usable_agent_command(&["agent".to_string()]));
    }
}
