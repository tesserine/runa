use std::path::Path;

use libagent::{InvalidArtifact, MalformedArtifact, PartiallyScannedType, UnreadableArtifact};

use super::CommandError;

pub fn run(working_dir: &Path, config_override: Option<&Path>) -> Result<(), CommandError> {
    let (loaded, result) = super::load_and_scan(working_dir, config_override)?;

    println!("Methodology: {}", loaded.manifest.name);
    println!("Workspace: {}", loaded.workspace_dir.display());
    println!();
    println!(
        "Summary: {} new, {} modified, {} revalidated, {} invalid, {} malformed, {} unreadable, {} partially scanned type{}, {} removed, {} unrecognized dir{}",
        result.new.len(),
        result.modified.len(),
        result.revalidated.len(),
        result.invalid.len(),
        result.malformed.len(),
        result.unreadable.len(),
        result.partially_scanned_types.len(),
        if result.partially_scanned_types.len() == 1 {
            ""
        } else {
            "s"
        },
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
    print_unreadable("Unreadable", &result.unreadable);
    print_partially_scanned_types("Partially scanned types", &result.partially_scanned_types);
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

fn print_unreadable(label: &str, artifacts: &[UnreadableArtifact]) {
    if artifacts.is_empty() {
        return;
    }

    println!();
    println!("{label}:");
    for artifact in artifacts {
        println!("  {}: {}", artifact.path.display(), artifact.error);
    }
}

fn print_partially_scanned_types(label: &str, types: &[PartiallyScannedType]) {
    if types.is_empty() {
        return;
    }

    println!();
    println!("{label}:");
    for partial in types {
        println!(
            "  type {} was only partially readable, {} entr{} could not be scanned, removal suppressed for this type.",
            partial.artifact_type,
            partial.unreadable_entries,
            if partial.unreadable_entries == 1 {
                "y"
            } else {
                "ies"
            }
        );
    }
}
