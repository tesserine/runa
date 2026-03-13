use std::fmt;
use std::path::Path;

use libagent::{InvalidArtifact, MalformedArtifact, ScanError as LibScanError};

use crate::project::{self, ProjectError};

#[derive(Debug)]
pub enum ScanError {
    Project(ProjectError),
    Scan(LibScanError),
}

impl fmt::Display for ScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScanError::Project(err) => write!(f, "{err}"),
            ScanError::Scan(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ScanError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ScanError::Project(err) => Some(err),
            ScanError::Scan(err) => Some(err),
        }
    }
}

pub fn run(working_dir: &Path, config_override: Option<&Path>) -> Result<(), ScanError> {
    let mut loaded = project::load(working_dir, config_override).map_err(ScanError::Project)?;
    let result =
        libagent::scan(&loaded.workspace_dir, &mut loaded.store).map_err(ScanError::Scan)?;

    println!("Methodology: {}", loaded.manifest.name);
    println!("Workspace: {}", loaded.workspace_dir.display());
    println!();
    println!(
        "Summary: {} new, {} modified, {} revalidated, {} invalid, {} malformed, {} removed, {} unrecognized dir{}",
        result.new.len(),
        result.modified.len(),
        result.revalidated.len(),
        result.invalid.len(),
        result.malformed.len(),
        result.removed.len(),
        result.unrecognized_dirs.len(),
        if result.unrecognized_dirs.len() == 1 {
            ""
        } else {
            "s"
        }
    );

    print_refs("New", &result.new);
    print_refs("Modified", &result.modified);
    print_refs("Revalidated", &result.revalidated);
    print_invalid("Invalid", &result.invalid);
    print_malformed("Malformed", &result.malformed);
    print_refs("Removed", &result.removed);

    if !result.unrecognized_dirs.is_empty() {
        println!();
        println!("Unrecognized directories:");
        for name in &result.unrecognized_dirs {
            println!("  {name}");
        }
    }

    Ok(())
}

fn print_refs(label: &str, artifacts: &[libagent::ArtifactRef]) {
    if artifacts.is_empty() {
        return;
    }

    println!();
    println!("{label}:");
    for artifact in artifacts {
        println!(
            "  {}/{} ({})",
            artifact.artifact_type,
            artifact.instance_id,
            artifact.path.display()
        );
    }
}

fn print_invalid(label: &str, artifacts: &[InvalidArtifact]) {
    if artifacts.is_empty() {
        return;
    }

    println!();
    println!("{label}:");
    for artifact in artifacts {
        println!(
            "  {}/{} ({})",
            artifact.artifact_type,
            artifact.instance_id,
            artifact.path.display()
        );
        for violation in &artifact.violations {
            println!("    - {}: {}", violation.schema_path, violation.description);
        }
    }
}

fn print_malformed(label: &str, artifacts: &[MalformedArtifact]) {
    if artifacts.is_empty() {
        return;
    }

    println!();
    println!("{label}:");
    for artifact in artifacts {
        println!(
            "  {}/{} ({}): {}",
            artifact.artifact_type,
            artifact.instance_id,
            artifact.path.display(),
            artifact.error
        );
    }
}
