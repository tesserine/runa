use std::fmt;
use std::path::Path;

use libagent::ValidationStatus;

use crate::project::{self, ProjectError};

#[derive(Debug)]
pub enum DoctorError {
    Project(ProjectError),
}

impl fmt::Display for DoctorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DoctorError::Project(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DoctorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DoctorError::Project(e) => Some(e),
        }
    }
}

/// Run the doctor command. Returns `true` if healthy, `false` if problems found.
pub fn run(working_dir: &Path) -> Result<bool, DoctorError> {
    let loaded = project::load(working_dir).map_err(DoctorError::Project)?;

    let mut problems = 0;

    println!("Methodology: {}", loaded.manifest.name);

    // --- Artifact health ---
    println!();
    println!("Artifacts:");

    let type_names = loaded.store.artifact_type_names();
    if type_names.is_empty() {
        println!("  (none)");
    }

    for type_name in &type_names {
        let instances = loaded.store.instances_of(type_name);
        if instances.is_empty() {
            println!("  {type_name}: no instances");
            continue;
        }

        let total = instances.len();
        let mut valid_count = 0;
        let mut invalid_count = 0;
        let mut stale_count = 0;

        for (_, state) in &instances {
            match &state.status {
                ValidationStatus::Valid => valid_count += 1,
                ValidationStatus::Invalid(_) => invalid_count += 1,
                ValidationStatus::Stale => stale_count += 1,
            }
        }

        if invalid_count == 0 && stale_count == 0 {
            println!(
                "  {type_name}: {total} instance{}, all valid",
                if total == 1 { "" } else { "s" }
            );
        } else {
            let mut parts = Vec::new();
            if valid_count > 0 {
                parts.push(format!("{valid_count} valid"));
            }
            if invalid_count > 0 {
                parts.push(format!("{invalid_count} invalid"));
            }
            if stale_count > 0 {
                parts.push(format!("{stale_count} stale"));
            }
            println!(
                "  {type_name}: {total} instance{} ({})",
                if total == 1 { "" } else { "s" },
                parts.join(", ")
            );

            // Report per-instance details for problems.
            for (instance_id, state) in &instances {
                match &state.status {
                    ValidationStatus::Invalid(violations) => {
                        problems += 1;
                        println!("    {instance_id}: invalid");
                        for v in violations {
                            println!("      - {}: {}", v.schema_path, v.description);
                        }
                    }
                    ValidationStatus::Stale => {
                        problems += 1;
                        println!("    {instance_id}: stale");
                    }
                    ValidationStatus::Valid => {}
                }
            }
        }
    }

    // --- Skill readiness ---
    println!();
    println!("Skills:");

    if loaded.manifest.skills.is_empty() {
        println!("  (none)");
    }

    for skill in &loaded.manifest.skills {
        if skill.requires.is_empty() {
            println!("  {}: ok", skill.name);
            continue;
        }

        let mut missing: Vec<&str> = Vec::new();
        let mut invalid: Vec<&str> = Vec::new();

        for req in &skill.requires {
            if !loaded.store.is_valid(req) {
                let instances = loaded.store.instances_of(req);
                if instances.is_empty() {
                    missing.push(req);
                } else if loaded.store.has_any_invalid(req) {
                    invalid.push(req);
                } else {
                    // Has instances but none valid (e.g., all stale).
                    missing.push(req);
                }
            }
        }

        if missing.is_empty() && invalid.is_empty() {
            println!("  {}: ok", skill.name);
        } else {
            problems += 1;
            let mut reasons = Vec::new();
            if !missing.is_empty() {
                reasons.push(format!("missing: {}", missing.join(", ")));
            }
            if !invalid.is_empty() {
                reasons.push(format!("invalid: {}", invalid.join(", ")));
            }
            println!("  {}: cannot execute ({})", skill.name, reasons.join("; "));
        }
    }

    // --- Cycle detection ---
    println!();
    match loaded.graph.topological_order() {
        Ok(_) => println!("Graph: no cycles"),
        Err(cycle) => {
            problems += 1;
            println!("Graph: {cycle}");
        }
    }

    println!();
    if problems == 0 {
        println!("No problems found.");
    } else {
        println!(
            "{} problem{} found.",
            problems,
            if problems == 1 { "" } else { "s" }
        );
    }

    Ok(problems == 0)
}
