use std::fmt;
use std::path::Path;

use libagent::context::{ArtifactRelationship, ContextInjection};
use serde::Serialize;

use crate::commands::protocol_eval;
use crate::project::{self, ProjectError};

const NOT_IMPLEMENTED_MESSAGE: &str =
    "Agent execution is not yet implemented. Use --dry-run to see the execution plan.";

#[derive(Debug)]
pub enum StepError {
    Project(ProjectError),
    Scan(libagent::ScanError),
    Json(serde_json::Error),
    NotImplemented,
}

impl fmt::Display for StepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StepError::Project(err) => write!(f, "{err}"),
            StepError::Scan(err) => write!(f, "{err}"),
            StepError::Json(err) => write!(f, "{err}"),
            StepError::NotImplemented => write!(f, "{NOT_IMPLEMENTED_MESSAGE}"),
        }
    }
}

impl std::error::Error for StepError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StepError::Project(err) => Some(err),
            StepError::Scan(err) => Some(err),
            StepError::Json(err) => Some(err),
            StepError::NotImplemented => None,
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
struct PlanEntry {
    protocol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    work_unit: Option<String>,
    trigger: String,
    context: ContextInjection,
}

pub fn run(
    working_dir: &Path,
    config_override: Option<&Path>,
    dry_run: bool,
    json_output: bool,
) -> Result<(), StepError> {
    if !dry_run {
        return Err(StepError::NotImplemented);
    }

    let mut loaded = project::load(working_dir, config_override).map_err(StepError::Project)?;
    let scan_result =
        libagent::scan(&loaded.workspace_dir, &mut loaded.store).map_err(StepError::Scan)?;
    let scan_findings = protocol_eval::collect_scan_findings(&scan_result, &loaded.workspace_dir);
    let evaluated = protocol_eval::evaluate_protocols(&loaded, working_dir, &scan_findings);
    let warnings = scan_findings.warnings.clone();

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
    let execution_plan: Vec<PlanEntry> = evaluated
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
        .collect();

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
