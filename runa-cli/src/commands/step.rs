use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fmt;
use std::io::{self, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitStatus, Stdio};
use std::thread;

use libagent::ExecutionRecord;
use libagent::context::{
    ArtifactRelationship, ContextInjection, ContextInjectionView, render_context_prompt,
};
use serde::{Serialize, Serializer};
use tracing::{info, warn};

use super::CommandError;
use crate::commands::protocol_eval;
use crate::exit_codes::ExitCode;

#[derive(Debug)]
pub enum StepError {
    Command(CommandError),
    Json(serde_json::Error),
    AgentCommandNotConfigured,
    JsonRequiresDryRun,
    McpBinaryNotFound {
        binary_name: String,
        sibling_path: Option<PathBuf>,
    },
    AgentCommandIo {
        command: String,
        stage: &'static str,
        source: io::Error,
    },
    AgentMcpConfigConflict {
        command: String,
    },
    AgentCommandFailed {
        command: String,
        protocol: String,
        work_unit: Option<String>,
        status: String,
    },
    PostExecutionScan {
        protocol: String,
        work_unit: Option<String>,
        source: libagent::ScanError,
    },
    PostExecutionEnforcement {
        protocol: String,
        work_unit: Option<String>,
        source: libagent::EnforcementError,
    },
    PostExecutionRecord {
        protocol: String,
        work_unit: Option<String>,
        source: libagent::StoreError,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepOutcome {
    Success,
    Blocked,
    NothingReady,
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
            StepError::McpBinaryNotFound {
                binary_name,
                sibling_path,
            } => match sibling_path {
                Some(path) => write!(
                    f,
                    "could not locate MCP server binary '{binary_name}' via sibling lookup ({}) or PATH",
                    path.display()
                ),
                None => write!(
                    f,
                    "could not locate MCP server binary '{binary_name}' via sibling lookup or PATH"
                ),
            },
            StepError::AgentCommandIo {
                command,
                stage,
                source,
            } => write!(
                f,
                "agent command '{command}' failed during {stage}: {source}"
            ),
            StepError::AgentMcpConfigConflict { command } => write!(
                f,
                "direct Claude command already supplies --mcp-config: '{command}'; runa provides the per-protocol MCP config automatically"
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
            StepError::PostExecutionScan {
                protocol,
                work_unit,
                source,
            } => match work_unit {
                Some(work_unit) => write!(
                    f,
                    "post-execution reconciliation failed for protocol '{protocol}' (work_unit={work_unit}): agent command succeeded but workspace re-scan failed: {source}"
                ),
                None => write!(
                    f,
                    "post-execution reconciliation failed for protocol '{protocol}': agent command succeeded but workspace re-scan failed: {source}"
                ),
            },
            StepError::PostExecutionEnforcement {
                protocol,
                work_unit,
                source,
            } => match work_unit {
                Some(work_unit) => write!(
                    f,
                    "post-execution reconciliation failed for protocol '{protocol}' (work_unit={work_unit}): agent command succeeded but protocol outputs did not satisfy the contract\n{source}"
                ),
                None => write!(
                    f,
                    "post-execution reconciliation failed for protocol '{protocol}': agent command succeeded but protocol outputs did not satisfy the contract\n{source}"
                ),
            },
            StepError::PostExecutionRecord {
                protocol,
                work_unit,
                source,
            } => match work_unit {
                Some(work_unit) => write!(
                    f,
                    "post-execution reconciliation failed for protocol '{protocol}' (work_unit={work_unit}): agent command succeeded but execution metadata could not be recorded: {source}"
                ),
                None => write!(
                    f,
                    "post-execution reconciliation failed for protocol '{protocol}': agent command succeeded but execution metadata could not be recorded: {source}"
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
            StepError::McpBinaryNotFound { .. } => None,
            StepError::AgentCommandIo { source, .. } => Some(source),
            StepError::AgentMcpConfigConflict { .. } => None,
            StepError::AgentCommandFailed { .. } => None,
            StepError::PostExecutionScan { source, .. } => Some(source),
            StepError::PostExecutionEnforcement { source, .. } => Some(source),
            StepError::PostExecutionRecord { source, .. } => Some(source),
        }
    }
}

impl From<CommandError> for StepError {
    fn from(err: CommandError) -> Self {
        StepError::Command(err)
    }
}

impl StepError {
    pub(crate) fn exit_code(&self) -> ExitCode {
        match self {
            StepError::JsonRequiresDryRun | StepError::AgentMcpConfigConflict { .. } => {
                ExitCode::UsageError
            }
            StepError::AgentCommandFailed { .. } | StepError::PostExecutionEnforcement { .. } => {
                ExitCode::WorkFailed
            }
            StepError::Command(_)
            | StepError::Json(_)
            | StepError::AgentCommandNotConfigured
            | StepError::McpBinaryNotFound { .. }
            | StepError::AgentCommandIo { .. }
            | StepError::PostExecutionScan { .. }
            | StepError::PostExecutionRecord { .. } => ExitCode::InfrastructureFailure,
        }
    }
}

impl StepOutcome {
    pub(crate) const fn exit_code(self) -> ExitCode {
        match self {
            StepOutcome::Success => ExitCode::Success,
            StepOutcome::Blocked => ExitCode::Blocked,
            StepOutcome::NothingReady => ExitCode::NothingReady,
        }
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
pub(crate) struct PlanEntry {
    pub(crate) protocol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) work_unit: Option<String>,
    pub(crate) trigger: String,
    pub(crate) mcp_config: McpServerConfig,
    #[serde(serialize_with = "serialize_context")]
    pub(crate) context: ContextInjection,
    #[serde(skip)]
    pub(crate) execution_record: ExecutionRecord,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct McpServerConfig {
    pub(crate) command: String,
    pub(crate) args: Vec<String>,
    pub(crate) env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlannedEntry {
    pub(crate) protocol: String,
    pub(crate) work_unit: Option<String>,
    pub(crate) trigger: String,
    pub(crate) context: ContextInjection,
    pub(crate) execution_record: ExecutionRecord,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ExecutionOptions {
    pub(crate) isolate_process_group: bool,
}

struct BinaryLookup {
    sibling_path: Option<PathBuf>,
    resolved_path: Option<PathBuf>,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CandidateKey {
    protocol: String,
    work_unit: Option<String>,
}

pub(crate) struct ExecutionState {
    pub(crate) scan_findings: protocol_eval::ScanFindings,
    pub(crate) evaluated: protocol_eval::EvaluatedProtocols,
    pub(crate) planned_entries: Vec<PlannedEntry>,
}

fn serialize_context<S>(context: &ContextInjection, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    ContextInjectionView::from(context).serialize(serializer)
}

fn has_blocked_work(evaluated: &protocol_eval::EvaluatedProtocols) -> bool {
    evaluated.cycle.is_some()
        || !evaluated.blocked.is_empty()
        || evaluated
            .waiting
            .iter()
            .any(|entry| entry.waiting_reason != Some(libagent::WaitingReason::OutputsCurrent))
}

fn classify_no_ready_outcome(evaluated: &protocol_eval::EvaluatedProtocols) -> StepOutcome {
    if has_blocked_work(evaluated) {
        StepOutcome::Blocked
    } else {
        StepOutcome::NothingReady
    }
}

fn decide_live_fallback_after_refresh(
    refreshed: ExecutionState,
) -> Result<PlannedEntry, StepOutcome> {
    match refreshed.planned_entries.into_iter().next() {
        Some(entry) => Ok(entry),
        None => Err(classify_no_ready_outcome(&refreshed.evaluated)),
    }
}

pub(crate) fn evaluate_execution_state(
    loaded: &crate::project::LoadedProject,
    working_dir: &Path,
    scan_result: &libagent::ScanResult,
    scope: libagent::EvaluationScope<'_>,
) -> ExecutionState {
    let scan_findings = protocol_eval::collect_scan_findings(scan_result, &loaded.workspace_dir);
    let evaluated = protocol_eval::evaluate_protocols(loaded, working_dir, &scan_findings, scope);
    let planned_entries = build_execution_plan(loaded, &scan_findings, &evaluated);

    ExecutionState {
        scan_findings,
        evaluated,
        planned_entries,
    }
}

pub(crate) fn build_execution_plan(
    loaded: &crate::project::LoadedProject,
    scan_findings: &protocol_eval::ScanFindings,
    evaluated: &protocol_eval::EvaluatedProtocols,
) -> Vec<PlannedEntry> {
    let protocol_map: std::collections::HashMap<&str, &libagent::ProtocolDeclaration> = loaded
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
            PlannedEntry {
                protocol: entry.name.clone(),
                work_unit: entry.work_unit.clone(),
                trigger: protocol.trigger.to_string(),
                context,
                execution_record: libagent::protocol_execution_record(
                    protocol,
                    &loaded.store,
                    entry.work_unit.as_deref(),
                    &scan_findings.affected_types,
                ),
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

fn is_direct_claude_command(command: &str) -> bool {
    Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == binary_executable_name("claude"))
}

fn command_has_mcp_config_arg(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--mcp-config")
}

fn write_claude_mcp_config(
    command_display: &str,
    mcp_config: &McpServerConfig,
) -> Result<tempfile::NamedTempFile, StepError> {
    let mut file = tempfile::Builder::new()
        .prefix("runa-claude-mcp-")
        .suffix(".json")
        .tempfile()
        .map_err(|source| StepError::AgentCommandIo {
            command: command_display.to_string(),
            stage: "mcp_config_create",
            source,
        })?;
    serde_json::to_writer(
        &mut file,
        &serde_json::json!({
            "mcpServers": {
                "runa": mcp_config
            }
        }),
    )
    .map_err(StepError::Json)?;
    file.flush().map_err(|source| StepError::AgentCommandIo {
        command: command_display.to_string(),
        stage: "mcp_config_write",
        source,
    })?;
    Ok(file)
}

#[cfg(test)]
fn candidate_key(protocol: &str, work_unit: Option<&str>) -> CandidateKey {
    CandidateKey {
        protocol: protocol.to_string(),
        work_unit: work_unit.map(str::to_owned),
    }
}

pub(crate) fn execute_entry(
    working_dir: &Path,
    agent_command: &[String],
    entry: &PlanEntry,
    options: ExecutionOptions,
) -> Result<(), StepError> {
    let command_display = format_command(agent_command);
    let transcript_capture_enabled = libagent::transcript::capture_enabled();
    info!(
        operation = "agent_execution",
        outcome = "starting",
        protocol = %entry.protocol,
        work_unit = ?entry.work_unit,
        command = %command_display,
        "starting agent command"
    );

    let direct_claude_command = is_direct_claude_command(&agent_command[0]);
    if direct_claude_command && command_has_mcp_config_arg(&agent_command[1..]) {
        return Err(StepError::AgentMcpConfigConflict {
            command: command_display,
        });
    }
    let claude_mcp_config = if direct_claude_command {
        Some(write_claude_mcp_config(
            &command_display,
            &entry.mcp_config,
        )?)
    } else {
        None
    };

    let mut child = ProcessCommand::new(&agent_command[0]);
    if options.isolate_process_group {
        child.process_group(0);
    }
    if let Some(config) = &claude_mcp_config {
        child
            .arg("--mcp-config")
            .arg(config.path())
            .arg("--strict-mcp-config");
    }
    child.args(&agent_command[1..]).env(
        "RUNA_MCP_CONFIG",
        serde_json::to_string(&entry.mcp_config).map_err(StepError::Json)?,
    );
    child.current_dir(working_dir).stdin(Stdio::piped());
    if transcript_capture_enabled {
        child.stdout(Stdio::piped()).stderr(Stdio::piped());
    } else {
        child.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    }

    let mut child = child.spawn().map_err(|source| StepError::AgentCommandIo {
        command: command_display.clone(),
        stage: "spawn",
        source,
    })?;

    let stream_forwarders = if transcript_capture_enabled {
        let stdout = child
            .stdout
            .take()
            .expect("agent stdout should be piped for transcript capture");
        let stderr = child
            .stderr
            .take()
            .expect("agent stderr should be piped for transcript capture");
        Some((
            spawn_stream_forwarder(
                stdout,
                "stdout",
                entry.protocol.clone(),
                entry.work_unit.clone(),
            ),
            spawn_stream_forwarder(
                stderr,
                "stderr",
                entry.protocol.clone(),
                entry.work_unit.clone(),
            ),
        ))
    } else {
        None
    };

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| StepError::AgentCommandIo {
                command: command_display.clone(),
                stage: "stdin_open",
                source: io::Error::new(io::ErrorKind::BrokenPipe, "child stdin was not available"),
            })?;
        let prompt = render_context_prompt(&entry.context);
        if transcript_capture_enabled {
            libagent::transcript::append_event(libagent::transcript::TranscriptEvent {
                source: "runa",
                kind: "agent_input",
                protocol: Some(&entry.protocol),
                work_unit: entry.work_unit.as_deref(),
                content: Some(&prompt),
                ..Default::default()
            })
            .map_err(|source| StepError::AgentCommandIo {
                command: command_display.clone(),
                stage: "transcript_write",
                source,
            })?;
        }
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
    if let Some((stdout_forwarder, stderr_forwarder)) = stream_forwarders {
        finish_stream_forwarder(stdout_forwarder, &command_display)?;
        finish_stream_forwarder(stderr_forwarder, &command_display)?;
    }

    if transcript_capture_enabled {
        libagent::transcript::append_event(libagent::transcript::TranscriptEvent {
            source: "runa",
            kind: "agent_exit",
            protocol: Some(&entry.protocol),
            work_unit: entry.work_unit.as_deref(),
            exit_code: status.code(),
            success: Some(status.success()),
            ..Default::default()
        })
        .map_err(|source| StepError::AgentCommandIo {
            command: command_display.clone(),
            stage: "transcript_write",
            source,
        })?;
    }

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
            command: command_display,
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

    Ok(())
}

fn spawn_stream_forwarder(
    stream: impl Read + Send + 'static,
    stream_name: &'static str,
    protocol: String,
    work_unit: Option<String>,
) -> thread::JoinHandle<io::Result<()>> {
    thread::spawn(move || {
        forward_stream_to_host_and_transcript(stream, stream_name, protocol, work_unit)
    })
}

fn finish_stream_forwarder(
    forwarder: thread::JoinHandle<io::Result<()>>,
    command_display: &str,
) -> Result<(), StepError> {
    forwarder
        .join()
        .map_err(|panic_payload| StepError::AgentCommandIo {
            command: command_display.to_string(),
            stage: "stream_forward",
            source: io::Error::other(thread_panic_message(panic_payload)),
        })?
        .map_err(|source| StepError::AgentCommandIo {
            command: command_display.to_string(),
            stage: "stream_forward",
            source,
        })
}

fn thread_panic_message(payload: Box<dyn std::any::Any + Send + 'static>) -> String {
    match payload.downcast::<String>() {
        Ok(message) => *message,
        Err(payload) => match payload.downcast::<&'static str>() {
            Ok(message) => (*message).to_string(),
            Err(_) => "unknown panic".to_string(),
        },
    }
}

fn forward_stream_to_host_and_transcript(
    mut stream: impl Read,
    stream_name: &'static str,
    protocol: String,
    work_unit: Option<String>,
) -> io::Result<()> {
    let mut buffer = [0_u8; 4096];

    loop {
        let bytes_read = stream.read(&mut buffer)?;
        if bytes_read == 0 {
            return Ok(());
        }

        let chunk = &buffer[..bytes_read];
        if stream_name == "stderr" {
            let mut host = io::stderr().lock();
            host.write_all(chunk)?;
            host.flush()?;
        } else {
            let mut host = io::stdout().lock();
            host.write_all(chunk)?;
            host.flush()?;
        }

        let content = String::from_utf8_lossy(chunk);
        libagent::transcript::append_event(libagent::transcript::TranscriptEvent {
            source: "runa",
            kind: if stream_name == "stderr" {
                "agent_stderr"
            } else {
                "agent_stdout"
            },
            protocol: Some(&protocol),
            work_unit: work_unit.as_deref(),
            stream: Some(stream_name),
            content: Some(&content),
            ..Default::default()
        })?;
    }
}

fn execute_live_single(
    working_dir: &Path,
    agent_command: &[String],
    config_path: &Path,
    loaded: &mut crate::project::LoadedProject,
    planned_entries: Vec<PlannedEntry>,
    scope: libagent::EvaluationScope<'_>,
) -> Result<StepOutcome, StepError> {
    let next_entry =
        match planned_entries.into_iter().next() {
            Some(entry) => entry,
            None => {
                let work_unit = match scope {
                    libagent::EvaluationScope::Scoped(work_unit) => Some(work_unit.to_owned()),
                    libagent::EvaluationScope::Unscoped => None,
                };
                let scan_result = libagent::scan(&loaded.workspace_dir, &mut loaded.store)
                    .map_err(|source| StepError::PostExecutionScan {
                        protocol: "<state-evaluation>".to_string(),
                        work_unit,
                        source,
                    })?;
                let refreshed = evaluate_execution_state(loaded, working_dir, &scan_result, scope);
                match decide_live_fallback_after_refresh(refreshed) {
                    Ok(entry) => entry,
                    Err(outcome) => {
                        println!("No READY protocols.");
                        return Ok(outcome);
                    }
                }
            }
        };

    let mcp_binary = locate_runa_mcp()?;
    let mcp_command = mcp_binary.to_string_lossy().into_owned();
    let execution_entry =
        build_plan_entries(vec![next_entry], &mcp_command, working_dir, config_path)
            .into_iter()
            .next()
            .expect("single planned entry must produce one execution entry");

    execute_entry(
        working_dir,
        agent_command,
        &execution_entry,
        ExecutionOptions::default(),
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
    libagent::enforce_postconditions(
        protocol,
        &loaded.store,
        execution_entry.work_unit.as_deref(),
    )
    .map_err(|source| StepError::PostExecutionEnforcement {
        protocol: execution_entry.protocol.clone(),
        work_unit: execution_entry.work_unit.clone(),
        source,
    })?;

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

    let refreshed = evaluate_execution_state(loaded, working_dir, &scan_result, scope);

    match &execution_entry.work_unit {
        Some(work_unit) => println!(
            "Executed: {} (work_unit={work_unit})",
            execution_entry.protocol
        ),
        None => println!("Executed: {}", execution_entry.protocol),
    }
    println!();
    protocol_eval::print_group("READY", &refreshed.evaluated.ready);
    println!();
    protocol_eval::print_group("BLOCKED", &refreshed.evaluated.blocked);
    println!();
    protocol_eval::print_group("WAITING", &refreshed.evaluated.waiting);

    Ok(StepOutcome::Success)
}

fn run_internal(
    working_dir: &Path,
    config_override: Option<&Path>,
    dry_run: bool,
    json_output: bool,
    single_dry_run: bool,
    work_unit: Option<&str>,
) -> Result<StepOutcome, StepError> {
    if !dry_run && json_output {
        return Err(StepError::JsonRequiresDryRun);
    }

    let (mut loaded, scan_result) = super::load_and_scan(working_dir, config_override)?;
    let scope = match work_unit {
        Some(work_unit) => libagent::EvaluationScope::Scoped(work_unit),
        None => libagent::EvaluationScope::Unscoped,
    };
    let ExecutionState {
        scan_findings,
        evaluated,
        planned_entries,
    } = evaluate_execution_state(&loaded, working_dir, &scan_result, scope);
    let warnings = scan_findings.warnings.clone();
    let config_path = crate::project::resolve_config(working_dir, config_override)
        .map_err(CommandError::from)
        .map_err(StepError::from)?;

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
        return execute_live_single(
            working_dir,
            agent_command
                .as_ref()
                .expect("live execution requires agent command"),
            &config_path,
            &mut loaded,
            planned_entries,
            scope,
        );
    }

    let planned_entries = if single_dry_run {
        planned_entries.into_iter().take(1).collect()
    } else {
        planned_entries
    };
    let execution_plan = build_plan_entries(
        planned_entries,
        &preview_runa_mcp_command(),
        working_dir,
        &config_path,
    );

    let execution_plan_is_empty = execution_plan.is_empty();

    if json_output {
        let payload = StepJson {
            version: 4,
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
                    serde_json::to_string_pretty(&ContextInjectionView::from(&entry.context))
                        .map_err(StepError::Json)?;
                for line in context.lines() {
                    println!("       {line}");
                }
                println!("     mcp_config:");
                let mcp_config =
                    serde_json::to_string_pretty(&entry.mcp_config).map_err(StepError::Json)?;
                for line in mcp_config.lines() {
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

    if execution_plan_is_empty {
        Ok(classify_no_ready_outcome(&evaluated))
    } else {
        Ok(StepOutcome::Success)
    }
}

pub fn run(
    working_dir: &Path,
    config_override: Option<&Path>,
    dry_run: bool,
    json_output: bool,
    work_unit: Option<&str>,
) -> Result<StepOutcome, StepError> {
    run_internal(
        working_dir,
        config_override,
        dry_run,
        json_output,
        true,
        work_unit,
    )
}

pub(crate) fn build_plan_entries(
    planned_entries: Vec<PlannedEntry>,
    mcp_command: &str,
    working_dir: &Path,
    config_path: &Path,
) -> Vec<PlanEntry> {
    planned_entries
        .into_iter()
        .map(|entry| PlanEntry {
            protocol: entry.protocol.clone(),
            work_unit: entry.work_unit.clone(),
            trigger: entry.trigger,
            mcp_config: build_mcp_config(
                mcp_command,
                working_dir,
                config_path,
                &entry.protocol,
                entry.work_unit.as_deref(),
            ),
            context: entry.context,
            execution_record: entry.execution_record,
        })
        .collect()
}

pub(crate) fn locate_runa_mcp() -> Result<PathBuf, StepError> {
    let executable_name = binary_executable_name("runa-mcp");
    let path_env = std::env::var_os("PATH");
    let lookup = discover_binary(std::env::current_exe(), path_env.as_deref(), "runa-mcp");

    lookup.resolved_path.ok_or(StepError::McpBinaryNotFound {
        binary_name: executable_name,
        sibling_path: lookup.sibling_path,
    })
}

pub(crate) fn preview_runa_mcp_command() -> String {
    let executable_name = binary_executable_name("runa-mcp");
    let path_env = std::env::var_os("PATH");
    let lookup = discover_binary(std::env::current_exe(), path_env.as_deref(), "runa-mcp");

    lookup
        .resolved_path
        .unwrap_or_else(|| PathBuf::from(executable_name))
        .to_string_lossy()
        .into_owned()
}

fn discover_binary(
    current_exe: Result<PathBuf, io::Error>,
    path_env: Option<&OsStr>,
    binary_name: &str,
) -> BinaryLookup {
    let sibling_path = current_exe
        .ok()
        .map(|path| sibling_binary_path(&path, binary_name));

    if sibling_path
        .as_ref()
        .is_some_and(|path| is_executable_binary(path))
    {
        return BinaryLookup {
            resolved_path: sibling_path.clone(),
            sibling_path,
        };
    }

    BinaryLookup {
        sibling_path,
        resolved_path: find_binary_on_path(path_env, binary_name),
    }
}

fn sibling_binary_path(current_exe: &Path, binary_name: &str) -> PathBuf {
    current_exe
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(binary_executable_name(binary_name))
}

fn find_binary_on_path(path_env: Option<&OsStr>, binary_name: &str) -> Option<PathBuf> {
    let executable_name = binary_executable_name(binary_name);
    let path_env = path_env?;
    for directory in std::env::split_paths(path_env) {
        let candidate = directory.join(&executable_name);
        if is_executable_binary(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable_binary(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.is_file()
        && std::fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

fn binary_executable_name(binary_name: &str) -> String {
    format!("{binary_name}{}", std::env::consts::EXE_SUFFIX)
}

fn build_mcp_config(
    mcp_command: &str,
    working_dir: &Path,
    config_path: &Path,
    protocol: &str,
    work_unit: Option<&str>,
) -> McpServerConfig {
    let mut args = vec!["--protocol".to_string(), protocol.to_string()];
    if let Some(work_unit) = work_unit {
        args.push("--work-unit".to_string());
        args.push(work_unit.to_string());
    }

    let working_dir = absolutize_path(working_dir, working_dir);
    let config_path = absolutize_path(config_path, &working_dir);
    let mut env = BTreeMap::new();
    env.insert(
        "RUNA_CONFIG".to_string(),
        config_path.to_string_lossy().into_owned(),
    );
    env.insert(
        "RUNA_WORKING_DIR".to_string(),
        working_dir.to_string_lossy().into_owned(),
    );
    env.extend(libagent::transcript::transcript_env());

    McpServerConfig {
        command: normalize_mcp_command(mcp_command, &working_dir),
        args,
        env,
    }
}

fn normalize_mcp_command(command: &str, working_dir: &Path) -> String {
    let command_path = Path::new(command);
    if command_path.is_absolute() {
        return command.to_string();
    }

    // Preserve the dry-run preview fallback when the binary is unresolved.
    if command_path.components().count() == 1 && !working_dir.join(command_path).exists() {
        return command.to_string();
    }

    absolutize_path(command_path, working_dir)
        .to_string_lossy()
        .into_owned()
}

fn absolutize_path(path: &Path, base_dir: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    };

    if absolute.exists() {
        std::fs::canonicalize(&absolute).unwrap_or(absolute)
    } else {
        absolute
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::{Mutex, OnceLock};

    fn transcript_env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct EnvGuard {
        previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl EnvGuard {
        fn set(values: &[(&'static str, &str)]) -> Self {
            let previous = values
                .iter()
                .map(|(name, _)| (*name, std::env::var_os(name)))
                .collect::<Vec<_>>();
            for (name, value) in values {
                unsafe { std::env::set_var(name, value) };
            }
            Self { previous }
        }

        fn unset(names: &[&'static str]) -> Self {
            let previous = names
                .iter()
                .map(|name| (*name, std::env::var_os(name)))
                .collect::<Vec<_>>();
            for name in names {
                unsafe { std::env::remove_var(name) };
            }
            Self { previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (name, value) in &self.previous {
                match value {
                    Some(value) => unsafe { std::env::set_var(name, value) },
                    None => unsafe { std::env::remove_var(name) },
                }
            }
        }
    }

    fn waiting_entry(
        unsatisfied_conditions: &[&str],
        waiting_reason: libagent::WaitingReason,
    ) -> protocol_eval::ProtocolEntry {
        protocol_eval::ProtocolEntry {
            name: "implement".to_string(),
            work_unit: None,
            status: protocol_eval::ProtocolStatus::Waiting,
            trigger: protocol_eval::TriggerState::NotSatisfied,
            inputs: Vec::new(),
            precondition_failures: Vec::new(),
            unsatisfied_conditions: unsatisfied_conditions
                .iter()
                .map(|condition| (*condition).to_string())
                .collect(),
            waiting_reason: Some(waiting_reason),
        }
    }

    fn empty_execution_state(
        waiting: Vec<protocol_eval::ProtocolEntry>,
        blocked: Vec<protocol_eval::ProtocolEntry>,
    ) -> ExecutionState {
        ExecutionState {
            scan_findings: protocol_eval::ScanFindings {
                affected_types: std::collections::HashSet::new(),
                warnings: Vec::new(),
            },
            evaluated: protocol_eval::EvaluatedProtocols {
                topology: libagent::EvaluationTopology {
                    status_order: Vec::new(),
                    execution_order: Vec::new(),
                    cycle: None,
                },
                cycle: None,
                ready: Vec::new(),
                blocked,
                waiting,
            },
            planned_entries: Vec::new(),
        }
    }

    fn write_executable_file(path: &Path) {
        use std::os::unix::fs::PermissionsExt;

        fs::write(path, b"").unwrap();
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }

    fn minimal_plan_entry(protocol: &str, work_unit: Option<&str>) -> PlanEntry {
        PlanEntry {
            protocol: protocol.to_string(),
            work_unit: work_unit.map(str::to_string),
            trigger: "on_artifact(request)".to_string(),
            mcp_config: McpServerConfig {
                command: "runa-mcp".to_string(),
                args: vec!["--protocol".to_string(), protocol.to_string()],
                env: BTreeMap::new(),
            },
            context: libagent::context::ContextInjection {
                protocol: protocol.to_string(),
                work_unit: work_unit.map(str::to_string),
                instructions: "Tell the operator about SECRET_VALUE.".to_string(),
                inputs: Vec::new(),
                expected_outputs: libagent::context::ExpectedOutputs {
                    produces: vec!["claim".to_string()],
                    may_produce: Vec::new(),
                },
            },
            execution_record: ExecutionRecord {
                input_modes: BTreeMap::new(),
                inputs: Default::default(),
            },
        }
    }

    fn write_fd_report_agent(path: &Path) {
        fs::write(
            path,
            r#"#!/bin/sh
set -eu
cat >/dev/null
exec 3>"$1"
readlink "/proc/$$/fd/1" >&3
readlink "/proc/$$/fd/2" >&3
exec 3>&-
"#,
        )
        .unwrap();
    }

    fn write_fake_claude(path: &Path) {
        fs::write(
            path,
            r#"#!/bin/sh
set -eu
printf '%s\n' "$@" > "$FAKE_CLAUDE_ARGV_CAPTURE"
config=""
while [ "$#" -gt 0 ]; do
    if [ "$1" = "--mcp-config" ]; then
        shift
        config="$1"
    fi
    shift
done
if [ -z "$config" ]; then
    exit 37
fi
cat "$config" > "$FAKE_CLAUDE_CONFIG_CAPTURE"
cat >/dev/null
"#,
        )
        .unwrap();
    }

    #[test]
    fn execute_entry_wires_direct_claude_command_to_mcp_config() {
        use std::os::unix::fs::PermissionsExt;

        let _lock = transcript_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let bin_dir = temp.path().join("bin");
        fs::create_dir(&bin_dir).unwrap();
        let claude = bin_dir.join("claude");
        write_fake_claude(&claude);
        let mut permissions = fs::metadata(&claude).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&claude, permissions).unwrap();
        let argv_capture = temp.path().join("argv.txt");
        let config_capture = temp.path().join("config.json");
        let path = format!(
            "{}:{}",
            bin_dir.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let _env = EnvGuard::set(&[
            ("PATH", &path),
            ("FAKE_CLAUDE_ARGV_CAPTURE", &argv_capture.to_string_lossy()),
            (
                "FAKE_CLAUDE_CONFIG_CAPTURE",
                &config_capture.to_string_lossy(),
            ),
        ]);

        execute_entry(
            temp.path(),
            &[
                "claude".to_string(),
                "-p".to_string(),
                "--dangerously-skip-permissions".to_string(),
            ],
            &minimal_plan_entry("implement", Some("issue-148")),
            ExecutionOptions::default(),
        )
        .expect("direct Claude execution should be wired to MCP config");

        let argv = fs::read_to_string(argv_capture).expect("fake claude should capture argv");
        assert!(argv.contains("--mcp-config\n"), "{argv}");
        assert!(argv.contains("--strict-mcp-config\n"), "{argv}");
        assert!(argv.contains("-p\n"), "{argv}");
        assert!(argv.contains("--dangerously-skip-permissions\n"), "{argv}");

        let config: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(config_capture).unwrap()).unwrap();
        assert_eq!(
            config,
            serde_json::json!({
                "mcpServers": {
                    "runa": {
                        "command": "runa-mcp",
                        "args": ["--protocol", "implement"],
                        "env": {}
                    }
                }
            })
        );
    }

    #[test]
    fn execute_entry_rejects_direct_claude_command_with_existing_mcp_config() {
        use std::os::unix::fs::PermissionsExt;

        let _lock = transcript_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let bin_dir = temp.path().join("bin");
        fs::create_dir(&bin_dir).unwrap();
        let claude = bin_dir.join("claude");
        write_fake_claude(&claude);
        let mut permissions = fs::metadata(&claude).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&claude, permissions).unwrap();
        let argv_capture = temp.path().join("argv.txt");
        let config_capture = temp.path().join("config.json");
        let path = format!(
            "{}:{}",
            bin_dir.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let _env = EnvGuard::set(&[
            ("PATH", &path),
            ("FAKE_CLAUDE_ARGV_CAPTURE", &argv_capture.to_string_lossy()),
            (
                "FAKE_CLAUDE_CONFIG_CAPTURE",
                &config_capture.to_string_lossy(),
            ),
        ]);

        let err = execute_entry(
            temp.path(),
            &[
                "claude".to_string(),
                "--mcp-config".to_string(),
                "operator-config.json".to_string(),
                "-p".to_string(),
            ],
            &minimal_plan_entry("implement", Some("issue-148")),
            ExecutionOptions::default(),
        )
        .expect_err("direct Claude execution should reject an existing MCP config");

        assert!(
            err.to_string()
                .contains("direct Claude command already supplies --mcp-config"),
            "{err}"
        );
        assert!(
            !argv_capture.exists(),
            "runa should fail before launching Claude"
        );
    }

    #[test]
    fn execute_entry_writes_redacted_agent_transcript_events_when_enabled() {
        let _lock = transcript_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let agent = temp.path().join("agent.sh");
        fs::write(
            &agent,
            "#!/bin/sh\nset -eu\ncat > prompt.txt\nprintf 'stdout SECRET_VALUE\\n'\nprintf 'stderr SECRET_VALUE\\n' >&2\n",
        )
        .unwrap();
        let transcript_dir = temp.path().join("transcript");
        let transcript_dir_string = transcript_dir.to_string_lossy().into_owned();
        let _env = EnvGuard::set(&[
            ("RUNA_TRANSCRIPT_DIR", &transcript_dir_string),
            ("RUNA_TRANSCRIPT_REDACT_ENV", "SECRET_TOKEN"),
            ("SECRET_TOKEN", "SECRET_VALUE"),
        ]);

        execute_entry(
            temp.path(),
            &["/bin/sh".to_string(), agent.to_string_lossy().into_owned()],
            &minimal_plan_entry("implement", Some("issue-116")),
            ExecutionOptions::default(),
        )
        .expect("agent execution should succeed");

        let events = fs::read_to_string(transcript_dir.join("events.jsonl"))
            .expect("transcript events should be written");
        assert!(events.contains("\"kind\":\"agent_input\""));
        assert!(events.contains("\"kind\":\"agent_stdout\""));
        assert!(events.contains("\"kind\":\"agent_stderr\""));
        assert!(events.contains("\"kind\":\"agent_exit\""));
        assert!(events.contains("[REDACTED:SECRET_TOKEN]"));
        assert!(
            !events.contains("SECRET_VALUE"),
            "secret values must not be persisted: {events}"
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn execute_entry_preserves_inherited_agent_streams_when_transcripts_are_disabled() {
        let _lock = transcript_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let agent = temp.path().join("agent.sh");
        let fd_report = temp.path().join("fd-report.txt");
        write_fd_report_agent(&agent);
        let _env = EnvGuard::unset(&["RUNA_TRANSCRIPT_DIR", "RUNA_TRANSCRIPT_REDACT_ENV"]);
        let parent_stdout = fs::read_link("/proc/self/fd/1").unwrap();
        let parent_stderr = fs::read_link("/proc/self/fd/2").unwrap();

        execute_entry(
            temp.path(),
            &[
                "/bin/sh".to_string(),
                agent.to_string_lossy().into_owned(),
                fd_report.to_string_lossy().into_owned(),
            ],
            &minimal_plan_entry("implement", Some("issue-116")),
            ExecutionOptions::default(),
        )
        .expect("agent execution should succeed");

        let report = fs::read_to_string(fd_report).expect("agent should report stream fds");
        let mut lines = report.lines();
        assert_eq!(
            lines.next().map(PathBuf::from),
            Some(parent_stdout),
            "agent stdout should be inherited unchanged when transcripts are disabled"
        );
        assert_eq!(
            lines.next().map(PathBuf::from),
            Some(parent_stderr),
            "agent stderr should be inherited unchanged when transcripts are disabled"
        );
        assert_eq!(lines.next(), None);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn execute_entry_pipes_agent_streams_when_transcripts_are_enabled() {
        let _lock = transcript_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let agent = temp.path().join("agent.sh");
        let fd_report = temp.path().join("fd-report.txt");
        write_fd_report_agent(&agent);
        let transcript_dir = temp.path().join("transcript");
        let transcript_dir_string = transcript_dir.to_string_lossy().into_owned();
        let _env = EnvGuard::set(&[("RUNA_TRANSCRIPT_DIR", &transcript_dir_string)]);
        let parent_stdout = fs::read_link("/proc/self/fd/1").unwrap();
        let parent_stderr = fs::read_link("/proc/self/fd/2").unwrap();

        execute_entry(
            temp.path(),
            &[
                "/bin/sh".to_string(),
                agent.to_string_lossy().into_owned(),
                fd_report.to_string_lossy().into_owned(),
            ],
            &minimal_plan_entry("implement", Some("issue-116")),
            ExecutionOptions::default(),
        )
        .expect("agent execution should succeed");

        let report = fs::read_to_string(fd_report).expect("agent should report stream fds");
        let mut lines = report.lines();
        let child_stdout = lines.next().expect("agent should report stdout fd");
        let child_stderr = lines.next().expect("agent should report stderr fd");
        assert!(
            child_stdout.starts_with("pipe:["),
            "agent stdout should be a pipe when transcripts are enabled: {child_stdout}"
        );
        assert!(
            child_stderr.starts_with("pipe:["),
            "agent stderr should be a pipe when transcripts are enabled: {child_stderr}"
        );
        assert_ne!(Path::new(child_stdout), parent_stdout.as_path());
        assert_ne!(Path::new(child_stderr), parent_stderr.as_path());
        assert_eq!(lines.next(), None);
    }

    #[test]
    fn candidate_key_preserves_protocol_and_work_unit() {
        let candidate = candidate_key("prepare", Some("wu-a"));

        assert_eq!(candidate.protocol, "prepare");
        assert_eq!(candidate.work_unit.as_deref(), Some("wu-a"));
    }

    #[test]
    fn build_mcp_config_includes_protocol_scope_and_project_env() {
        let config = build_mcp_config(
            "/tmp/bin/runa-mcp",
            Path::new("/tmp/project"),
            Path::new("/tmp/project/.runa/config.toml"),
            "implement",
            Some("wu-a"),
        );

        assert_eq!(config.command, "/tmp/bin/runa-mcp");
        assert_eq!(
            config.args,
            vec!["--protocol", "implement", "--work-unit", "wu-a"]
        );
        assert_eq!(
            config.env.get("RUNA_WORKING_DIR"),
            Some(&"/tmp/project".to_string())
        );
        assert_eq!(
            config.env.get("RUNA_CONFIG"),
            Some(&"/tmp/project/.runa/config.toml".to_string())
        );
    }

    #[test]
    fn build_mcp_config_forwards_transcript_environment_when_enabled() {
        let _lock = transcript_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _env = EnvGuard::set(&[
            ("RUNA_TRANSCRIPT_DIR", "/tmp/runa-transcript"),
            ("RUNA_TRANSCRIPT_REDACT_ENV", "SECRET_TOKEN"),
        ]);

        let config = build_mcp_config(
            "/tmp/bin/runa-mcp",
            Path::new("/tmp/project"),
            Path::new("/tmp/project/.runa/config.toml"),
            "implement",
            None,
        );

        assert_eq!(
            config.env.get("RUNA_TRANSCRIPT_DIR"),
            Some(&"/tmp/runa-transcript".to_string())
        );
        assert_eq!(
            config.env.get("RUNA_TRANSCRIPT_REDACT_ENV"),
            Some(&"SECRET_TOKEN".to_string())
        );
    }

    #[test]
    fn build_mcp_config_absolutizes_relative_command_and_config_paths() {
        let temp = tempfile::tempdir().unwrap();
        let working_dir = temp.path().join("project");
        let runa_dir = working_dir.join(".runa");
        let bin_dir = working_dir.join("target/debug");
        fs::create_dir_all(&runa_dir).unwrap();
        fs::create_dir_all(&bin_dir).unwrap();
        fs::write(runa_dir.join("config.toml"), "methodology_path = \"x\"").unwrap();
        write_executable_file(&bin_dir.join(binary_executable_name("runa-mcp")));

        let config = build_mcp_config(
            "target/debug/runa-mcp",
            &working_dir,
            Path::new(".runa/config.toml"),
            "implement",
            None,
        );

        assert_eq!(
            config.command,
            working_dir
                .join("target/debug")
                .join(binary_executable_name("runa-mcp"))
                .to_string_lossy()
                .into_owned()
        );
        assert_eq!(
            config.env.get("RUNA_WORKING_DIR"),
            Some(&working_dir.to_string_lossy().into_owned())
        );
        assert_eq!(
            config.env.get("RUNA_CONFIG"),
            Some(
                &working_dir
                    .join(".runa/config.toml")
                    .to_string_lossy()
                    .into_owned()
            )
        );
    }

    #[test]
    fn build_mcp_config_preserves_unresolved_bare_preview_command() {
        let config = build_mcp_config(
            "runa-mcp",
            Path::new("/tmp/project"),
            Path::new("/tmp/project/.runa/config.toml"),
            "implement",
            None,
        );

        assert_eq!(config.command, "runa-mcp");
    }

    #[test]
    fn discover_binary_prefers_sibling_over_path() {
        let temp = tempfile::tempdir().unwrap();
        let sibling_dir = temp.path().join("sibling");
        let path_dir = temp.path().join("path");
        std::fs::create_dir_all(&sibling_dir).unwrap();
        std::fs::create_dir_all(&path_dir).unwrap();

        let current_exe = sibling_dir.join(binary_executable_name("runa"));
        let sibling = sibling_dir.join(binary_executable_name("runa-mcp"));
        let path_binary = path_dir.join(binary_executable_name("runa-mcp"));
        write_executable_file(&sibling);
        write_executable_file(&path_binary);

        let path_env = std::env::join_paths([path_dir.as_path()]).unwrap();
        let lookup = discover_binary(Ok(current_exe), Some(path_env.as_os_str()), "runa-mcp");

        assert_eq!(lookup.sibling_path, Some(sibling.clone()));
        assert_eq!(lookup.resolved_path, Some(sibling));
    }

    #[test]
    fn discover_binary_uses_path_when_sibling_is_missing() {
        let temp = tempfile::tempdir().unwrap();
        let sibling_dir = temp.path().join("sibling");
        let path_dir = temp.path().join("path");
        std::fs::create_dir_all(&sibling_dir).unwrap();
        std::fs::create_dir_all(&path_dir).unwrap();

        let current_exe = sibling_dir.join(binary_executable_name("runa"));
        let sibling = sibling_dir.join(binary_executable_name("runa-mcp"));
        let path_binary = path_dir.join(binary_executable_name("runa-mcp"));
        write_executable_file(&path_binary);

        let path_env = std::env::join_paths([path_dir.as_path()]).unwrap();
        let lookup = discover_binary(Ok(current_exe), Some(path_env.as_os_str()), "runa-mcp");

        assert_eq!(lookup.sibling_path, Some(sibling));
        assert_eq!(lookup.resolved_path, Some(path_binary));
    }

    #[test]
    fn discover_binary_uses_path_when_current_exe_is_unavailable() {
        let temp = tempfile::tempdir().unwrap();
        let path_dir = temp.path().join("path");
        std::fs::create_dir_all(&path_dir).unwrap();

        let path_binary = path_dir.join(binary_executable_name("runa-mcp"));
        write_executable_file(&path_binary);

        let path_env = std::env::join_paths([path_dir.as_path()]).unwrap();
        let lookup = discover_binary(
            Err(io::Error::other("no current exe")),
            Some(path_env.as_os_str()),
            "runa-mcp",
        );

        assert_eq!(lookup.sibling_path, None);
        assert_eq!(lookup.resolved_path, Some(path_binary));
    }

    #[test]
    fn discover_binary_skips_non_executable_sibling_and_uses_path() {
        let temp = tempfile::tempdir().unwrap();
        let sibling_dir = temp.path().join("sibling");
        let path_dir = temp.path().join("path");
        std::fs::create_dir_all(&sibling_dir).unwrap();
        std::fs::create_dir_all(&path_dir).unwrap();

        let current_exe = sibling_dir.join(binary_executable_name("runa"));
        let sibling = sibling_dir.join(binary_executable_name("runa-mcp"));
        let path_binary = path_dir.join(binary_executable_name("runa-mcp"));
        std::fs::write(&sibling, b"").unwrap();
        write_executable_file(&path_binary);

        let path_env = std::env::join_paths([path_dir.as_path()]).unwrap();
        let lookup = discover_binary(Ok(current_exe), Some(path_env.as_os_str()), "runa-mcp");

        assert_eq!(lookup.sibling_path, Some(sibling));
        assert_eq!(lookup.resolved_path, Some(path_binary));
    }

    #[test]
    fn discover_binary_skips_non_executable_path_entries() {
        let temp = tempfile::tempdir().unwrap();
        let first_path_dir = temp.path().join("path-1");
        let second_path_dir = temp.path().join("path-2");
        std::fs::create_dir_all(&first_path_dir).unwrap();
        std::fs::create_dir_all(&second_path_dir).unwrap();

        let first_candidate = first_path_dir.join(binary_executable_name("runa-mcp"));
        let second_candidate = second_path_dir.join(binary_executable_name("runa-mcp"));
        std::fs::write(&first_candidate, b"").unwrap();
        write_executable_file(&second_candidate);

        let path_env =
            std::env::join_paths([first_path_dir.as_path(), second_path_dir.as_path()]).unwrap();
        let lookup = discover_binary(
            Err(io::Error::other("no current exe")),
            Some(path_env.as_os_str()),
            "runa-mcp",
        );

        assert_eq!(lookup.sibling_path, None);
        assert_eq!(lookup.resolved_path, Some(second_candidate));
    }

    #[test]
    fn live_fallback_reselects_ready_entry_after_refresh() {
        let planned = PlannedEntry {
            protocol: "implement".to_string(),
            work_unit: None,
            trigger: "on_artifact(constraints)".to_string(),
            context: libagent::context::ContextInjection {
                protocol: "implement".to_string(),
                work_unit: None,
                instructions: String::new(),
                inputs: Vec::new(),
                expected_outputs: libagent::context::ExpectedOutputs {
                    produces: vec!["implementation".to_string()],
                    may_produce: Vec::new(),
                },
            },
            execution_record: ExecutionRecord {
                input_modes: std::collections::BTreeMap::new(),
                inputs: Default::default(),
            },
        };
        let refreshed = ExecutionState {
            scan_findings: protocol_eval::ScanFindings {
                affected_types: std::collections::HashSet::new(),
                warnings: Vec::new(),
            },
            evaluated: protocol_eval::EvaluatedProtocols {
                topology: libagent::EvaluationTopology {
                    status_order: Vec::new(),
                    execution_order: Vec::new(),
                    cycle: None,
                },
                cycle: None,
                ready: Vec::new(),
                blocked: Vec::new(),
                waiting: Vec::new(),
            },
            planned_entries: vec![planned.clone()],
        };

        let decision = decide_live_fallback_after_refresh(refreshed);

        assert_eq!(decision, Ok(planned));
    }

    #[test]
    fn live_fallback_reports_blocked_when_refresh_still_has_no_ready_work() {
        let refreshed = empty_execution_state(
            vec![waiting_entry(
                &["constraints missing"],
                libagent::WaitingReason::TriggerUnsatisfied,
            )],
            Vec::new(),
        );

        let decision = decide_live_fallback_after_refresh(refreshed);

        assert_eq!(decision, Err(StepOutcome::Blocked));
    }

    #[test]
    fn live_fallback_reports_nothing_ready_when_refresh_only_has_current_outputs() {
        let refreshed = empty_execution_state(
            vec![waiting_entry(
                &["outputs already current"],
                libagent::WaitingReason::OutputsCurrent,
            )],
            Vec::new(),
        );

        let decision = decide_live_fallback_after_refresh(refreshed);

        assert_eq!(decision, Err(StepOutcome::NothingReady));
    }
}
