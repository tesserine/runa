use std::fmt;
use std::path::Path;

use serde::Serialize;

use crate::commands::protocol_eval;
use crate::project::{self, ProjectError};

#[derive(Debug)]
pub enum StatusError {
    Project(ProjectError),
    Scan(libagent::ScanError),
    Json(serde_json::Error),
}

impl fmt::Display for StatusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StatusError::Project(err) => write!(f, "{err}"),
            StatusError::Scan(err) => write!(f, "{err}"),
            StatusError::Json(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for StatusError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StatusError::Project(err) => Some(err),
            StatusError::Scan(err) => Some(err),
            StatusError::Json(err) => Some(err),
        }
    }
}

#[derive(Serialize)]
struct StatusJson<'a> {
    version: u32,
    methodology: &'a str,
    scan_warnings: Vec<String>,
    protocols: Vec<protocol_eval::ProtocolJson>,
}

pub fn run(
    working_dir: &Path,
    config_override: Option<&Path>,
    json_output: bool,
) -> Result<(), StatusError> {
    let mut loaded = project::load(working_dir, config_override).map_err(StatusError::Project)?;
    let scan_result =
        libagent::scan(&loaded.workspace_dir, &mut loaded.store).map_err(StatusError::Scan)?;
    let scan_findings = protocol_eval::collect_scan_findings(&scan_result, &loaded.workspace_dir);
    let evaluated = protocol_eval::evaluate_protocols(&loaded, working_dir, &scan_findings);
    let warnings = scan_findings.warnings.clone();

    if json_output {
        let payload = StatusJson {
            version: 2,
            methodology: &loaded.manifest.name,
            scan_warnings: warnings.clone(),
            protocols: evaluated.json_protocols(),
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).map_err(StatusError::Json)?
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
