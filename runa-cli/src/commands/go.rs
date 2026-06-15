use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use crate::commands::step::{
    ExecutionOptions, PlanEntry, StepError, StepOutcome, build_session_mcp_config,
    build_session_ticket_mcp_config, execute_entry, locate_runa_mcp,
};
use crate::commands::{CommandError, entry, protocol_eval};

fn configured_agent_command(
    working_dir: &Path,
    config_override: Option<&Path>,
) -> Result<Vec<String>, StepError> {
    let config = crate::project::read_config(working_dir, config_override)
        .map_err(CommandError::from)
        .map_err(StepError::from)?;
    config
        .agent
        .command
        .filter(|command| {
            !command.is_empty() && !command.first().is_some_and(|part| part.is_empty())
        })
        .ok_or(StepError::AgentCommandNotConfigured)
}

fn tick_prompt() -> &'static str {
    "Advance this runa session by exactly one step.\n\
     Call the runa MCP tool `next-protocol-context`, follow the returned rendered prompt, \
     produce the required output through the current runa MCP output tool, call `advance`, \
     and then stop. Do not call readiness as a separate operator action."
}

fn receipt_io_error(stage: &'static str, source: io::Error) -> StepError {
    StepError::AgentCommandIo {
        command: "go".to_string(),
        stage,
        source,
    }
}

fn session_advance_receipt_matches(
    receipt_path: &Path,
    protocol: &str,
    work_unit: Option<&str>,
) -> Result<bool, StepError> {
    let receipt = match fs::read_to_string(receipt_path) {
        Ok(receipt) => receipt,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(receipt_io_error("session_advance_receipt_read", error)),
    };
    let receipt: serde_json::Value = serde_json::from_str(&receipt).map_err(StepError::Json)?;
    let completed_step = &receipt["completed_step"];
    Ok(completed_step["protocol"].as_str() == Some(protocol)
        && completed_step["work_unit"].as_str() == work_unit)
}

pub fn run(
    working_dir: &Path,
    config_override: Option<&Path>,
    work_unit: Option<&str>,
    ticket: Option<&str>,
) -> Result<StepOutcome, StepError> {
    match (work_unit, ticket) {
        (Some(work_unit), _) => run_bound(working_dir, config_override, work_unit),
        (None, Some(ticket)) => run_ticket(working_dir, config_override, ticket),
        (None, None) => unreachable!("clap requires --work-unit or --ticket"),
    }
}

/// Advance a session opened from a forge ticket reference by one tick.
///
/// Re-entry (the work-unit already exists) degrades to a normal bound tick.
/// Otherwise this tick is the acquisition step: the runtime serves the
/// methodology's acquisition surface, the agent materializes the work-unit, and
/// the session binds — leaving `take` ready on the acquired work-unit.
fn run_ticket(
    working_dir: &Path,
    config_override: Option<&Path>,
    ticket: &str,
) -> Result<StepOutcome, StepError> {
    let (loaded, scan_result) = super::load_and_scan(working_dir, config_override)?;
    let ticket_ref = entry::resolve_reference(&loaded, ticket)?;

    // Re-entry (the work-unit already exists) degrades to a bound session and
    // needs no acquisition surface — resolve before discovering it.
    if let Some(work_unit) = entry::resolve_existing(&loaded, &ticket_ref)? {
        println!(
            "Ticket {} resolves to recorded work-unit {work_unit}",
            ticket_ref.display
        );
        return run_bound(working_dir, config_override, &work_unit);
    }

    let acquisition = entry::acquisition_surface(&loaded)?;

    // Entry substitutes only the trigger; the acquisition's preconditions and
    // scan trust still gate. Block before launching the agent when unmet.
    if entry::acquisition_block_reason(&loaded, &acquisition, &scan_result).is_some() {
        println!("No READY protocols.");
        return Ok(StepOutcome::Blocked);
    }

    // The agent is required only once the ticket and acquisition surface are
    // validated, so a bad reference or unsupported manifest reports its own
    // error class rather than a missing-agent config failure.
    let agent_command = configured_agent_command(working_dir, config_override)?;
    let config_path = crate::project::resolve_config(working_dir, config_override)
        .map_err(CommandError::from)
        .map_err(StepError::from)?;
    let runtime_env = entry::entry_runtime_env(working_dir, &loaded.config, &ticket_ref)?;
    let transcript_settings = libagent::transcript::resolve_transcript_settings_with_forge(
        working_dir,
        &loaded.config.transcript,
        &loaded.config.deployment,
        &loaded.config.forge,
    );
    let mcp_binary = locate_runa_mcp()?;
    let receipt_dir = tempfile::Builder::new()
        .prefix("runa-go-advance-")
        .tempdir()
        .map_err(|source| receipt_io_error("session_advance_receipt_create", source))?;
    let receipt_path = receipt_dir.path().join("advance.json");
    let receipt_path_env = receipt_path.to_string_lossy().into_owned();
    let mut mcp_config = build_session_ticket_mcp_config(
        &mcp_binary.to_string_lossy(),
        working_dir,
        &config_path,
        ticket,
        &runtime_env,
    );
    mcp_config.env.insert(
        libagent::SESSION_ADVANCE_RECEIPT_ENV.to_string(),
        receipt_path_env.clone(),
    );
    let plan_entry = PlanEntry {
        protocol: "go".to_string(),
        work_unit: None,
        trigger: "ticket_entry".to_string(),
        mcp_config,
        context: libagent::context::ContextInjection {
            protocol: "go".to_string(),
            work_unit: None,
            instructions: tick_prompt().to_string(),
            inputs: Vec::new(),
            entry: None,
            expected_outputs: libagent::context::ExpectedOutputs {
                produces: Vec::new(),
                may_produce: Vec::new(),
                required_output_choices: Vec::new(),
            },
        },
        execution_record: libagent::ExecutionRecord {
            input_modes: BTreeMap::new(),
            inputs: Default::default(),
        },
    };

    execute_entry(
        working_dir,
        &agent_command,
        &plan_entry,
        ExecutionOptions {
            extra_env: runtime_env
                .into_iter()
                .chain([(
                    libagent::SESSION_ADVANCE_RECEIPT_ENV.to_string(),
                    receipt_path_env,
                )])
                .collect(),
            transcript_settings: Some(transcript_settings),
            ..ExecutionOptions::default()
        },
    )?;

    if !session_advance_receipt_matches(&receipt_path, &acquisition.name, None)? {
        return Err(StepError::SessionDidNotAdvance {
            protocol: acquisition.name.clone(),
            work_unit: None,
        });
    }

    let mut loaded = crate::project::load(working_dir, config_override)
        .map_err(CommandError::from)
        .map_err(StepError::from)?;
    let scan_result =
        libagent::scan(&loaded.workspace_dir, &mut loaded.store).map_err(|source| {
            StepError::PostExecutionScan {
                protocol: acquisition.name.clone(),
                work_unit: None,
                source,
            }
        })?;
    let work_unit = entry::resolve_existing(&loaded, &ticket_ref)?.ok_or_else(|| {
        StepError::TicketReference(libagent::EntryError::Unresolved {
            reference: ticket_ref.display.clone(),
        })
    })?;

    let scope = libagent::EvaluationScope::Scoped(&work_unit);
    let refreshed =
        crate::commands::step::evaluate_execution_state(&loaded, working_dir, &scan_result, scope);

    println!(
        "Acquired work-unit {work_unit} from ticket {}",
        ticket_ref.display
    );
    println!();
    protocol_eval::print_group("READY", &refreshed.evaluated.ready);
    println!();
    protocol_eval::print_group("BLOCKED", &refreshed.evaluated.blocked);
    println!();
    protocol_eval::print_group("WAITING", &refreshed.evaluated.waiting);

    Ok(StepOutcome::Success)
}

fn run_bound(
    working_dir: &Path,
    config_override: Option<&Path>,
    work_unit: &str,
) -> Result<StepOutcome, StepError> {
    let agent_command = configured_agent_command(working_dir, config_override)?;
    let (loaded, scan_result) = super::load_and_scan(working_dir, config_override)?;
    super::validate_scoped_work_unit(&loaded, Some(work_unit)).map_err(StepError::from)?;
    let scope = libagent::EvaluationScope::Scoped(work_unit);
    let state =
        crate::commands::step::evaluate_execution_state(&loaded, working_dir, &scan_result, scope);

    if state.planned_entries.is_empty() {
        println!("No READY protocols.");
        if state.evaluated.cycle.is_some()
            || !state.evaluated.blocked.is_empty()
            || state
                .evaluated
                .waiting
                .iter()
                .any(|entry| entry.waiting_reason != Some(libagent::WaitingReason::OutputsCurrent))
        {
            return Ok(StepOutcome::Blocked);
        }
        return Ok(StepOutcome::NothingReady);
    }
    let selected_step = state
        .planned_entries
        .first()
        .expect("non-empty plan should have a selected step");

    let config_path = crate::project::resolve_config(working_dir, config_override)
        .map_err(CommandError::from)
        .map_err(StepError::from)?;
    let runtime_env = crate::commands::step::resolved_runtime_env(working_dir, &loaded.config)?;
    let transcript_settings = libagent::transcript::resolve_transcript_settings_with_forge(
        working_dir,
        &loaded.config.transcript,
        &loaded.config.deployment,
        &loaded.config.forge,
    );
    let mcp_binary = locate_runa_mcp()?;
    let receipt_dir = tempfile::Builder::new()
        .prefix("runa-go-advance-")
        .tempdir()
        .map_err(|source| receipt_io_error("session_advance_receipt_create", source))?;
    let receipt_path = receipt_dir.path().join("advance.json");
    let receipt_path_env = receipt_path.to_string_lossy().into_owned();
    let mut mcp_config = build_session_mcp_config(
        &mcp_binary.to_string_lossy(),
        working_dir,
        &config_path,
        work_unit,
        &runtime_env,
    );
    mcp_config.env.insert(
        libagent::SESSION_ADVANCE_RECEIPT_ENV.to_string(),
        receipt_path_env.clone(),
    );
    let entry = PlanEntry {
        protocol: "go".to_string(),
        work_unit: Some(work_unit.to_string()),
        trigger: "session_tick".to_string(),
        mcp_config,
        context: libagent::context::ContextInjection {
            protocol: "go".to_string(),
            work_unit: Some(work_unit.to_string()),
            instructions: tick_prompt().to_string(),
            inputs: Vec::new(),
            entry: None,
            expected_outputs: libagent::context::ExpectedOutputs {
                produces: Vec::new(),
                may_produce: Vec::new(),
                required_output_choices: Vec::new(),
            },
        },
        execution_record: libagent::ExecutionRecord {
            input_modes: BTreeMap::new(),
            inputs: Default::default(),
        },
    };

    execute_entry(
        working_dir,
        &agent_command,
        &entry,
        ExecutionOptions {
            extra_env: runtime_env
                .into_iter()
                .chain([(
                    libagent::SESSION_ADVANCE_RECEIPT_ENV.to_string(),
                    receipt_path_env,
                )])
                .collect(),
            transcript_settings: Some(transcript_settings),
            ..ExecutionOptions::default()
        },
    )?;

    if !session_advance_receipt_matches(
        &receipt_path,
        &selected_step.protocol,
        selected_step.work_unit.as_deref(),
    )? {
        return Err(StepError::SessionDidNotAdvance {
            protocol: selected_step.protocol.clone(),
            work_unit: selected_step.work_unit.clone(),
        });
    }

    let mut loaded = crate::project::load(working_dir, config_override)
        .map_err(CommandError::from)
        .map_err(StepError::from)?;
    let scan_result =
        libagent::scan(&loaded.workspace_dir, &mut loaded.store).map_err(|source| {
            StepError::PostExecutionScan {
                protocol: "go".to_string(),
                work_unit: Some(work_unit.to_string()),
                source,
            }
        })?;
    let refreshed =
        crate::commands::step::evaluate_execution_state(&loaded, working_dir, &scan_result, scope);

    println!("Advanced one session step (work_unit={work_unit})");
    println!();
    protocol_eval::print_group("READY", &refreshed.evaluated.ready);
    println!();
    protocol_eval::print_group("BLOCKED", &refreshed.evaluated.blocked);
    println!();
    protocol_eval::print_group("WAITING", &refreshed.evaluated.waiting);

    Ok(StepOutcome::Success)
}
