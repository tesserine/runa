use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fmt;
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};
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
    McpBinaryNotFound {
        binary_name: String,
        sibling_path: Option<PathBuf>,
    },
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
            StepError::McpBinaryNotFound { .. } => None,
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
    mcp_config: McpServerConfig,
    context: ContextInjection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct McpServerConfig {
    command: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
}

struct PlannedEntry {
    protocol: String,
    work_unit: Option<String>,
    trigger: String,
    context: ContextInjection,
}

struct BinaryLookup {
    sibling_path: Option<PathBuf>,
    resolved_path: Option<PathBuf>,
}

fn build_execution_plan(
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

    let cycle_participants: std::collections::HashSet<&str> = evaluated
        .cycle
        .as_ref()
        .map(|cycle| cycle.path.iter().map(|name| name.as_str()).collect())
        .unwrap_or_default();

    let ready_entries: Vec<_> = evaluated
        .ready
        .iter()
        .filter(|entry| !cycle_participants.contains(entry.name.as_str()))
        .collect();

    if ready_entries.is_empty() {
        return Vec::new();
    }

    ready_entries
        .into_iter()
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
            .env(
                "RUNA_MCP_CONFIG",
                serde_json::to_string(&entry.mcp_config).map_err(StepError::Json)?,
            )
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
    let config_path = crate::project::resolve_config(working_dir, config_override)
        .map_err(CommandError::from)
        .map_err(StepError::from)?;
    let planned_entries = build_execution_plan(&loaded, &scan_findings, &evaluated);

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
        if planned_entries.is_empty() {
            println!("No READY protocols.");
            return Ok(());
        }
        let mcp_binary = locate_runa_mcp()?;
        let execution_plan = build_plan_entries(
            planned_entries,
            &mcp_binary.to_string_lossy(),
            working_dir,
            &config_path,
        );
        return execute_plan(
            working_dir,
            agent_command
                .as_ref()
                .expect("live execution requires agent command"),
            &execution_plan,
        );
    }

    let execution_plan = build_plan_entries(
        planned_entries,
        &preview_runa_mcp_command(),
        working_dir,
        &config_path,
    );

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

    Ok(())
}

fn build_plan_entries(
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
        })
        .collect()
}

fn locate_runa_mcp() -> Result<PathBuf, StepError> {
    let executable_name = binary_executable_name("runa-mcp");
    let path_env = std::env::var_os("PATH");
    let lookup = discover_binary(std::env::current_exe(), path_env.as_deref(), "runa-mcp");

    lookup
        .resolved_path
        .ok_or(StepError::McpBinaryNotFound {
            binary_name: executable_name,
            sibling_path: lookup.sibling_path,
        })
}

fn preview_runa_mcp_command() -> String {
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

    if sibling_path.as_ref().is_some_and(|path| path.is_file()) {
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
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
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
    fn build_mcp_config_absolutizes_relative_command_and_config_paths() {
        let temp = tempfile::tempdir().unwrap();
        let working_dir = temp.path().join("project");
        let runa_dir = working_dir.join(".runa");
        let bin_dir = working_dir.join("target/debug");
        fs::create_dir_all(&runa_dir).unwrap();
        fs::create_dir_all(&bin_dir).unwrap();
        fs::write(runa_dir.join("config.toml"), "methodology_path = \"x\"").unwrap();
        fs::write(bin_dir.join(binary_executable_name("runa-mcp")), b"").unwrap();

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
        std::fs::write(&sibling, b"").unwrap();
        std::fs::write(&path_binary, b"").unwrap();

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
        std::fs::write(&path_binary, b"").unwrap();

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
        std::fs::write(&path_binary, b"").unwrap();

        let path_env = std::env::join_paths([path_dir.as_path()]).unwrap();
        let lookup = discover_binary(
            Err(io::Error::other("no current exe")),
            Some(path_env.as_os_str()),
            "runa-mcp",
        );

        assert_eq!(lookup.sibling_path, None);
        assert_eq!(lookup.resolved_path, Some(path_binary));
    }
}
