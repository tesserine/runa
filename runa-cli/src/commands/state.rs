use std::fmt;
use std::path::Path;

use serde::Serialize;

use super::CommandError;
use crate::commands::protocol_eval;

#[derive(Debug)]
pub enum StateError {
    Command(CommandError),
    Json(serde_json::Error),
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StateError::Command(err) => write!(f, "{err}"),
            StateError::Json(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for StateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StateError::Command(err) => Some(err),
            StateError::Json(err) => Some(err),
        }
    }
}

impl From<CommandError> for StateError {
    fn from(err: CommandError) -> Self {
        StateError::Command(err)
    }
}

#[derive(Serialize)]
struct StateJson<'a> {
    version: u32,
    methodology: &'a str,
    scan_warnings: Vec<String>,
    protocols: Vec<protocol_eval::ProtocolJson>,
}

pub fn run(
    working_dir: &Path,
    config_override: Option<&Path>,
    json_output: bool,
    work_unit: Option<&str>,
) -> Result<(), StateError> {
    let (loaded, scan_result) = super::load_and_scan(working_dir, config_override)?;
    let scan_findings = protocol_eval::collect_scan_findings(&scan_result, &loaded.workspace_dir);
    let scope = match work_unit {
        Some(work_unit) => libagent::EvaluationScope::Scoped(work_unit),
        None => libagent::EvaluationScope::Unscoped,
    };
    let evaluated = protocol_eval::evaluate_protocols(&loaded, working_dir, &scan_findings, scope);
    let warnings = scan_findings.warnings.clone();

    if json_output {
        let payload = StateJson {
            version: 2,
            methodology: &loaded.manifest.name,
            scan_warnings: warnings.clone(),
            protocols: evaluated.json_protocols(),
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).map_err(StateError::Json)?
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
        protocol_eval::print_group("READY", &evaluated.ready);
        println!();
        protocol_eval::print_group("BLOCKED", &evaluated.blocked);
        println!();
        protocol_eval::print_group("WAITING", &evaluated.waiting);
    }

    Ok(())
}
