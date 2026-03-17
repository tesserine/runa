use std::fmt;
use std::path::Path;

use libagent::{
    ArtifactFailure, ScanError as StoreScanError, ValidationStatus, enforce_preconditions,
};

use crate::project::{self, ProjectError};

#[derive(Debug)]
pub enum DoctorError {
    Project(ProjectError),
    Scan(StoreScanError),
}

impl fmt::Display for DoctorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DoctorError::Project(e) => write!(f, "{e}"),
            DoctorError::Scan(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DoctorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DoctorError::Project(e) => Some(e),
            DoctorError::Scan(e) => Some(e),
        }
    }
}

/// Run the doctor command. Returns `true` if healthy, `false` if problems found.
pub fn run(working_dir: &Path, config_override: Option<&Path>) -> Result<bool, DoctorError> {
    let mut loaded = project::load(working_dir, config_override).map_err(DoctorError::Project)?;
    let scan_result =
        libagent::scan(&loaded.workspace_dir, &mut loaded.store).map_err(DoctorError::Scan)?;

    let mut problems = 0;

    println!("Methodology: {}", loaded.manifest.name);

    if !scan_result.unreadable.is_empty() {
        println!();
        println!("Scan:");
        for partial in &scan_result.partially_scanned_types {
            problems += 1;
            println!(
                "  partial: type {} was only partially readable, {} entr{} could not be scanned, removal suppressed for this type.",
                partial.artifact_type,
                partial.unreadable_entries,
                if partial.unreadable_entries == 1 {
                    "y"
                } else {
                    "ies"
                }
            );
        }
        for entry in &scan_result.unreadable {
            problems += 1;
            println!("  unreadable: {}", entry.path.display());
            println!("    {}", entry.error);
        }
    }

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
        let mut malformed_count = 0;
        let mut stale_count = 0;

        for (_, state) in &instances {
            match &state.status {
                ValidationStatus::Valid => valid_count += 1,
                ValidationStatus::Invalid(_) => invalid_count += 1,
                ValidationStatus::Malformed(_) => malformed_count += 1,
                ValidationStatus::Stale => stale_count += 1,
            }
        }

        if invalid_count == 0 && malformed_count == 0 && stale_count == 0 {
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
            if malformed_count > 0 {
                parts.push(format!("{malformed_count} malformed"));
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
                    ValidationStatus::Malformed(error) => {
                        problems += 1;
                        println!("    {instance_id}: malformed");
                        println!("      - {error}");
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
    println!("Protocols:");

    if loaded.manifest.protocols.is_empty() {
        println!("  (none)");
    }

    for protocol in &loaded.manifest.protocols {
        if protocol.requires.is_empty() {
            println!("  {}: ok", protocol.name);
            continue;
        }

        if let Err(err) = enforce_preconditions(protocol, &loaded.store) {
            problems += 1;
            println!(
                "  {}: cannot execute ({})",
                protocol.name,
                format_failures(&err.failures)
            );
        } else {
            println!("  {}: ok", protocol.name);
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

fn format_failures(failures: &[ArtifactFailure]) -> String {
    let missing = names_for(failures, |failure| {
        matches!(failure, ArtifactFailure::Missing { .. })
    });
    let invalid = names_for(failures, |failure| {
        matches!(failure, ArtifactFailure::Invalid { .. })
    });
    let stale = names_for(failures, |failure| {
        matches!(failure, ArtifactFailure::Stale { .. })
    });

    let mut reasons = Vec::new();
    if !missing.is_empty() {
        reasons.push(format!("missing: {}", missing.join(", ")));
    }
    if !invalid.is_empty() {
        reasons.push(format!("invalid: {}", invalid.join(", ")));
    }
    if !stale.is_empty() {
        reasons.push(format!("stale: {}", stale.join(", ")));
    }

    reasons.join("; ")
}

fn names_for(
    failures: &[ArtifactFailure],
    predicate: impl Fn(&ArtifactFailure) -> bool,
) -> Vec<String> {
    failures
        .iter()
        .filter(|failure| predicate(failure))
        .map(|failure| match failure {
            ArtifactFailure::Missing { artifact_type, .. }
            | ArtifactFailure::Invalid { artifact_type, .. }
            | ArtifactFailure::Stale { artifact_type, .. } => artifact_type.clone(),
        })
        .collect()
}
