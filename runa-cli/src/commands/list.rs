use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::Path;

use libagent::ScanError as StoreScanError;

use crate::project::{self, ProjectError};

#[derive(Debug)]
pub enum ListError {
    Project(ProjectError),
    Scan(StoreScanError),
}

impl fmt::Display for ListError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ListError::Project(e) => write!(f, "{e}"),
            ListError::Scan(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ListError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ListError::Project(e) => Some(e),
            ListError::Scan(e) => Some(e),
        }
    }
}

pub fn run(working_dir: &Path, config_override: Option<&Path>) -> Result<(), ListError> {
    let mut loaded = project::load(working_dir, config_override).map_err(ListError::Project)?;
    libagent::scan(&loaded.workspace_dir, &mut loaded.store).map_err(ListError::Scan)?;

    println!("Methodology: {}", loaded.manifest.name);

    // Get execution order. On cycle, fall back to manifest order with warning.
    let skill_order: Vec<&str> = match loaded.graph.topological_order() {
        Ok(order) => order,
        Err(cycle) => {
            eprintln!("warning: {cycle}");
            loaded
                .manifest
                .skills
                .iter()
                .map(|s| s.name.as_str())
                .collect()
        }
    };

    // Build available artifacts set from store.
    let available: HashSet<String> = loaded
        .manifest
        .artifact_types
        .iter()
        .filter(|at| loaded.store.is_valid(&at.name))
        .map(|at| at.name.clone())
        .collect();

    let blocked = loaded.graph.blocked_skills_with_reasons(&available);
    let blocked_map: HashMap<&str, Vec<&str>> = blocked.into_iter().collect();

    // Build a lookup from skill name to declaration.
    let skill_map: HashMap<&str, &libagent::SkillDeclaration> = loaded
        .manifest
        .skills
        .iter()
        .map(|s| (s.name.as_str(), s))
        .collect();

    println!("Skills (execution order):");

    for (i, &name) in skill_order.iter().enumerate() {
        let Some(skill) = skill_map.get(name) else {
            continue;
        };

        println!();
        println!("  {}. {}", i + 1, name);

        if !skill.requires.is_empty() {
            println!("     requires: {}", skill.requires.join(", "));
        }
        if !skill.accepts.is_empty() {
            println!("     accepts:  {}", skill.accepts.join(", "));
        }
        if !skill.produces.is_empty() {
            println!("     produces: {}", skill.produces.join(", "));
        }
        if !skill.may_produce.is_empty() {
            println!("     may_produce: {}", skill.may_produce.join(", "));
        }

        println!("     trigger:  {}", skill.trigger);

        if let Some(missing) = blocked_map.get(name) {
            let missing_list: Vec<String> = missing.iter().map(|m| format!("'{m}'")).collect();
            println!(
                "     BLOCKED:  missing artifact type {}",
                missing_list.join(", ")
            );
        }
    }

    Ok(())
}
