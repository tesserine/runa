use std::collections::BTreeMap;
use std::path::Path;

use crate::commands::step::{
    ExecutionOptions, PlanEntry, StepError, StepOutcome, build_session_mcp_config, execute_entry,
    locate_runa_mcp,
};
use crate::commands::{CommandError, protocol_eval};

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

pub fn run(
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
    let previous_execution_record = loaded
        .store
        .execution_record(&selected_step.protocol, selected_step.work_unit.as_deref())
        .cloned();

    let config_path = crate::project::resolve_config(working_dir, config_override)
        .map_err(CommandError::from)
        .map_err(StepError::from)?;
    let mcp_binary = locate_runa_mcp()?;
    let mcp_config = build_session_mcp_config(
        &mcp_binary.to_string_lossy(),
        working_dir,
        &config_path,
        work_unit,
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
        ExecutionOptions::default(),
    )?;

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
    let current_execution_record = loaded
        .store
        .execution_record(&selected_step.protocol, selected_step.work_unit.as_deref())
        .cloned();
    if current_execution_record == previous_execution_record {
        return Err(StepError::SessionDidNotAdvance {
            protocol: selected_step.protocol.clone(),
            work_unit: selected_step.work_unit.clone(),
        });
    }
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
