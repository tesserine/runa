use std::collections::HashMap;
use std::path::Path;

use libagent::ArtifactFailure;
use tracing::warn;

use super::CommandError;

pub fn run(working_dir: &Path, config_override: Option<&Path>) -> Result<(), CommandError> {
    let (loaded, _scan_result) = super::load_and_scan(working_dir, config_override)?;

    println!("Methodology: {}", loaded.manifest.name);

    // Get execution order. On cycle, fall back to manifest order with warning.
    let protocol_order: Vec<&str> = match loaded.graph.topological_order() {
        Ok(order) => order,
        Err(cycle) => {
            warn!(
                operation = "topological_order",
                outcome = "cycle_fallback",
                error = %cycle,
                "falling back to manifest order after cycle detection"
            );
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

        if let Err(err) = libagent::enforce_preconditions(protocol, &loaded.store, None) {
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
