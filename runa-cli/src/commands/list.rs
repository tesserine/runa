use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use libagent::ScanError as StoreScanError;
use libagent::{ArtifactFailure, enforce_preconditions};

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
    let protocol_order: Vec<&str> = match loaded.graph.topological_order() {
        Ok(order) => order,
        Err(cycle) => {
            eprintln!("warning: {cycle}");
            loaded
                .manifest
                .protocols
                .iter()
                .map(|s| s.name.as_str())
                .collect()
        }
    };

    // Build a lookup from protocol name to declaration.
    let protocol_map: HashMap<&str, &libagent::ProtocolDeclaration> = loaded
        .manifest
        .protocols
        .iter()
        .map(|s| (s.name.as_str(), s))
        .collect();

    println!("Protocols (execution order):");

    for (i, &name) in protocol_order.iter().enumerate() {
        let Some(protocol) = protocol_map.get(name) else {
            continue;
        };

        println!();
        println!("  {}. {}", i + 1, name);

        if !protocol.requires.is_empty() {
            println!("     requires: {}", protocol.requires.join(", "));
        }
        if !protocol.accepts.is_empty() {
            println!("     accepts:  {}", protocol.accepts.join(", "));
        }
        if !protocol.produces.is_empty() {
            println!("     produces: {}", protocol.produces.join(", "));
        }
        if !protocol.may_produce.is_empty() {
            println!("     may_produce: {}", protocol.may_produce.join(", "));
        }

        println!("     trigger:  {}", protocol.trigger);

        if let Err(err) = enforce_preconditions(protocol, &loaded.store, None) {
            println!("     BLOCKED:  {}", format_failures(&err.failures));
        }
    }

    Ok(())
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
        reasons.push(format!("missing: {}", quoted(&missing)));
    }
    if !invalid.is_empty() {
        reasons.push(format!("invalid: {}", quoted(&invalid)));
    }
    if !stale.is_empty() {
        reasons.push(format!("stale: {}", quoted(&stale)));
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

fn quoted(names: &[String]) -> String {
    names
        .iter()
        .map(|name| format!("'{name}'"))
        .collect::<Vec<_>>()
        .join(", ")
}
