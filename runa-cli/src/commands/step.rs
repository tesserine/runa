use std::fmt;
use std::io;
use std::io::Write;
use std::path::Path;
use std::process::{Command as ProcessCommand, ExitStatus, Stdio};

use libagent::context::{ArtifactRelationship, ContextInjection, render_context_prompt};
use serde::Serialize;
use tracing::{info, warn};

use super::CommandError;
use crate::commands::protocol_eval;

#[derive(Debug)]
pub enum StepError {
    Command(CommandError),
    Json(serde_json::Error),
    AgentCommandNotConfigured,
    JsonRequiresDryRun,
    AgentCommandIo {
        command: String,
        stage: &'static str,
        source: io::Error,
    },
    AgentCommandFailed {
        command: String,
        protocol: String,
        work_unit: Option<String>,
        status: String,
    },
}

impl fmt::Display for StepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StepError::Command(err) => write!(f, "{err}"),
            StepError::Json(err) => write!(f, "{err}"),
            StepError::AgentCommandNotConfigured => write!(
                f,
                "no agent command configured in config.toml; add [agent] command = [\"binary\", ...]"
            ),
            StepError::JsonRequiresDryRun => {
                write!(f, "--json is only supported with --dry-run")
            }
            StepError::AgentCommandIo {
                command,
                stage,
                source,
            } => write!(
                f,
                "agent command '{command}' failed during {stage}: {source}"
            ),
            StepError::AgentCommandFailed {
                command,
                protocol,
                work_unit,
                status,
            } => match work_unit {
                Some(work_unit) => write!(
                    f,
                    "agent command '{command}' failed for protocol '{protocol}' (work_unit={work_unit}): {status}"
                ),
                None => write!(
                    f,
                    "agent command '{command}' failed for protocol '{protocol}': {status}"
                ),
            },
        }
    }
}

impl std::error::Error for StepError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StepError::Command(err) => Some(err),
            StepError::Json(err) => Some(err),
            StepError::AgentCommandNotConfigured => None,
            StepError::JsonRequiresDryRun => None,
            StepError::AgentCommandIo { source, .. } => Some(source),
            StepError::AgentCommandFailed { .. } => None,
        }
    }
}

impl From<CommandError> for StepError {
    fn from(err: CommandError) -> Self {
        StepError::Command(err)
    }
}

#[derive(Serialize)]
struct StepJson<'a> {
    version: u32,
    methodology: &'a str,
    scan_warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cycle: Option<Vec<String>>,
    execution_plan: Vec<PlanEntry>,
    protocols: Vec<protocol_eval::ProtocolJson>,
}

#[derive(Serialize)]
struct PlanEntry {
    protocol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    work_unit: Option<String>,
    trigger: String,
    context: ContextInjection,
}

fn build_execution_plan(
    loaded: &crate::project::LoadedProject,
    scan_findings: &protocol_eval::ScanFindings,
    evaluated: &protocol_eval::EvaluatedProtocols,
) -> Vec<PlanEntry> {
    let protocol_map: std::collections::HashMap<&str, &libagent::ProtocolDeclaration> = loaded
        .manifest
        .protocols
        .iter()
        .map(|protocol| (protocol.name.as_str(), protocol))
        .collect();

    let cycle_participants: std::collections::HashSet<&str> = evaluated
        .cycle
        .as_ref()
        .map(|cycle| cycle.path.iter().map(|name| name.as_str()).collect())
        .unwrap_or_default();

    evaluated
        .ready
        .iter()
        .filter(|entry| !cycle_participants.contains(entry.name.as_str()))
        .map(|entry| {
            let protocol = protocol_map
                .get(entry.name.as_str())
                .expect("planned protocol must exist in manifest");
            let mut context = libagent::context::build_context(
                protocol,
                &loaded.store,
                entry.work_unit.as_deref(),
            );
            context.inputs.retain(|input| {
                input.relationship == ArtifactRelationship::Requires
                    || !scan_findings
                        .affected_types
                        .contains(input.artifact_type.as_str())
            });
            PlanEntry {
                protocol: entry.name.clone(),
                work_unit: entry.work_unit.clone(),
                trigger: protocol.trigger.to_string(),
                context,
            }
        })
        .collect()
}

fn format_command(command: &[String]) -> String {
    command.join(" ")
}

fn format_exit_status(status: ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exited with status {code}"),
        None => "terminated without an exit code".to_string(),
    }
}

fn execute_plan(
    working_dir: &Path,
    agent_command: &[String],
    execution_plan: &[PlanEntry],
) -> Result<(), StepError> {
    let command_display = format_command(agent_command);
    for entry in execution_plan {
        info!(
            operation = "agent_execution",
            outcome = "starting",
            protocol = %entry.protocol,
            work_unit = ?entry.work_unit,
            command = %command_display,
            "starting agent command"
        );

        let mut child = ProcessCommand::new(&agent_command[0]);
        child
            .args(&agent_command[1..])
            .current_dir(working_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        let mut child = child.spawn().map_err(|source| StepError::AgentCommandIo {
            command: command_display.clone(),
            stage: "spawn",
            source,
        })?;

        {
            let stdin = child
                .stdin
                .as_mut()
                .ok_or_else(|| StepError::AgentCommandIo {
                    command: command_display.clone(),
                    stage: "stdin_open",
                    source: io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "child stdin was not available",
                    ),
                })?;
            let prompt = render_context_prompt(&entry.context);
            stdin
                .write_all(prompt.as_bytes())
                .and_then(|_| stdin.write_all(b"\n"))
                .map_err(|source| StepError::AgentCommandIo {
                    command: command_display.clone(),
                    stage: "stdin_write",
                    source,
                })?;
        }

        let status = child.wait().map_err(|source| StepError::AgentCommandIo {
            command: command_display.clone(),
            stage: "wait",
            source,
        })?;

        if !status.success() {
            warn!(
                operation = "agent_execution",
                outcome = "failed",
                protocol = %entry.protocol,
                work_unit = ?entry.work_unit,
                command = %command_display,
                status = %format_exit_status(status),
                "agent command failed"
            );
            return Err(StepError::AgentCommandFailed {
                command: command_display.clone(),
                protocol: entry.protocol.clone(),
                work_unit: entry.work_unit.clone(),
                status: format_exit_status(status),
            });
        }

        info!(
            operation = "agent_execution",
            outcome = "succeeded",
            protocol = %entry.protocol,
            work_unit = ?entry.work_unit,
            command = %command_display,
            "agent command succeeded"
        );
    }

    Ok(())
}

pub fn run(
    working_dir: &Path,
    config_override: Option<&Path>,
    dry_run: bool,
    json_output: bool,
) -> Result<(), StepError> {
    if !dry_run && json_output {
        return Err(StepError::JsonRequiresDryRun);
    }

    let (loaded, scan_result) = super::load_and_scan(working_dir, config_override)?;
    let scan_findings = protocol_eval::collect_scan_findings(&scan_result, &loaded.workspace_dir);
    let evaluated = protocol_eval::evaluate_protocols(&loaded, working_dir, &scan_findings);
    let warnings = scan_findings.warnings.clone();
    let execution_plan = build_execution_plan(&loaded, &scan_findings, &evaluated);

    let agent_command = if dry_run {
        None
    } else {
        let config = crate::project::read_config(working_dir, config_override)
            .map_err(CommandError::from)
            .map_err(StepError::from)?;
        let command = config.agent.command.filter(|command| {
            !command.is_empty() && !command.first().is_some_and(|part| part.is_empty())
        });
        if command.is_none() {
            return Err(StepError::AgentCommandNotConfigured);
        }
        command
    };

    if !dry_run {
        if execution_plan.is_empty() {
            println!("No READY protocols.");
            return Ok(());
        }
        return execute_plan(
            working_dir,
            agent_command
                .as_ref()
                .expect("live execution requires agent command"),
            &execution_plan,
        );
    }

    if json_output {
        let payload = StepJson {
            version: 2,
            methodology: &loaded.manifest.name,
            scan_warnings: warnings.clone(),
            cycle: evaluated.cycle.as_ref().map(|cycle| cycle.path.clone()),
            execution_plan,
            protocols: evaluated.json_protocols(),
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).map_err(StepError::Json)?
        );
    } else {
        println!("Methodology: {}", loaded.manifest.name);
        if !warnings.is_empty() {
            println!();
            println!("Scan warnings:");
            for warning in &warnings {
                println!("  - {warning}");
            }
        }
        println!();

        if let Some(cycle) = &evaluated.cycle {
            println!("warning: {cycle}");
        }

        if execution_plan.is_empty() {
            println!("Execution plan: none");
            if evaluated.cycle.is_none() {
                println!("No READY protocols.");
            }
        } else {
            println!("Execution plan:");
            for (index, entry) in execution_plan.iter().enumerate() {
                println!();
                match &entry.work_unit {
                    Some(work_unit) => {
                        println!(
                            "  {}. {} (work_unit={work_unit})",
                            index + 1,
                            entry.protocol
                        )
                    }
                    None => println!("  {}. {}", index + 1, entry.protocol),
                }
                println!("     trigger: {}", entry.trigger);
                println!("     context:");
                let context =
                    serde_json::to_string_pretty(&entry.context).map_err(StepError::Json)?;
                for line in context.lines() {
                    println!("       {line}");
                }
            }
        }

        println!();
        protocol_eval::print_group("READY", &evaluated.ready);
        println!();
        protocol_eval::print_group("BLOCKED", &evaluated.blocked);
        println!();
        protocol_eval::print_group("WAITING", &evaluated.waiting);
    }

    Ok(())
}
