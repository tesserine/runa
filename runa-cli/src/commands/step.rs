use std::fmt;
use std::path::Path;

use libagent::context::{ArtifactRelationship, ContextInjection};
use serde::Serialize;

use crate::commands::skill_eval;
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
    cycle_detected: Option<bool>,
    execution_plan: Vec<PlanEntry>,
    skills: Vec<skill_eval::SkillJson>,
}

#[derive(Serialize)]
struct PlanEntry {
    skill: String,
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
    let scan_findings = skill_eval::collect_scan_findings(&scan_result, &loaded.workspace_dir);
    let evaluated = skill_eval::evaluate_skills(&loaded, working_dir, &scan_findings);

    let skill_map: std::collections::HashMap<&str, &libagent::SkillDeclaration> = loaded
        .manifest
        .skills
        .iter()
        .map(|skill| (skill.name.as_str(), skill))
        .collect();

    let execution_plan: Vec<PlanEntry> = if evaluated.has_cycle {
        Vec::new()
    } else {
        evaluated
            .ready
            .iter()
            .map(|entry| {
                let skill = skill_map
                    .get(entry.name.as_str())
                    .expect("ready skill must exist in manifest");
                let mut context = libagent::context::build_context(skill, &loaded.store);
                context.inputs.retain(|input| {
                    input.relationship == ArtifactRelationship::Requires
                        || !scan_findings
                            .affected_types
                            .contains(input.artifact_type.as_str())
                });
                PlanEntry {
                    skill: entry.name.clone(),
                    trigger: skill.trigger.to_string(),
                    context,
                }
            })
            .collect()
    };

    if json_output {
        let payload = StepJson {
            version: 1,
            methodology: &loaded.manifest.name,
            scan_warnings: scan_findings.warnings.clone(),
            cycle_detected: evaluated.has_cycle.then_some(true),
            execution_plan,
            skills: evaluated.json_skills(),
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).map_err(StepError::Json)?
        );
    } else {
        println!("Methodology: {}", loaded.manifest.name);
        if !scan_findings.warnings.is_empty() {
            println!();
            println!("Scan warnings:");
            for warning in &scan_findings.warnings {
                println!("  - {warning}");
            }
        }
        println!();

        if evaluated.has_cycle {
            if let Err(cycle) = loaded.graph.topological_order() {
                println!("warning: {cycle}");
            }
            println!("Execution plan: none");
            println!("Cannot produce execution plan: dependency cycle detected.");
        } else if execution_plan.is_empty() {
            println!("Execution plan: none");
            println!("No READY skills.");
        } else {
            println!("Execution plan:");
            for (index, entry) in execution_plan.iter().enumerate() {
                println!();
                println!("  {}. {}", index + 1, entry.skill);
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
        skill_eval::print_group("READY", &evaluated.ready);
        println!();
        skill_eval::print_group("BLOCKED", &evaluated.blocked);
        println!();
        skill_eval::print_group("WAITING", &evaluated.waiting);
    }

    Ok(())
}
