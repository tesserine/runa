mod context;
mod handler;

use libagent::project::{self, RUNA_DIR, STORE_DIRNAME};
use libagent::{
    ArtifactStore, configure_tracing, discover_ready_candidates, enforce_postconditions, scan,
};
use rmcp::service::ServiceExt;
use rmcp::transport::io;
use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::process;
use tracing::{error, info, warn};

use handler::RunaHandler;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(err) = configure_tracing(None) {
        let _ = writeln!(std::io::stderr(), "runa-mcp: {err}");
        process::exit(1);
    }

    if let Err(e) = run().await {
        error!(
            operation = "mcp_session",
            outcome = "failed",
            error = %e,
            "mcp session failed"
        );
        eprintln!("runa-mcp: {e}");
        process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Parse args: working_dir from RUNA_WORKING_DIR or cwd,
    //    config override from RUNA_CONFIG.
    let working_dir = match std::env::var("RUNA_WORKING_DIR") {
        Ok(dir) => PathBuf::from(dir),
        Err(_) => std::env::current_dir()?,
    };
    let config_override = std::env::var("RUNA_CONFIG").ok().map(PathBuf::from);
    let config_ref = config_override.as_deref();
    if let Ok(config) = project::read_config(&working_dir, config_ref) {
        configure_tracing(Some(&config.logging))?;
    }

    // 2. Load project.
    let mut loaded = project::load(&working_dir, config_ref)?;
    let runa_dir = working_dir.join(RUNA_DIR);

    // 3. Scan workspace.
    let scan_result = scan(&loaded.workspace_dir, &mut loaded.store)?;

    // 4. Collect partially scanned types.
    let partially_scanned: HashSet<String> = scan_result
        .partially_scanned_types
        .iter()
        .map(|p| p.artifact_type.clone())
        .collect();

    // 5. Discover ready candidates.
    let topo_order = match loaded.graph.topological_order() {
        Ok(order) => order,
        Err(cycle) => {
            warn!(
                operation = "topological_order",
                outcome = "cycle_fallback",
                error = %cycle,
                "falling back to orderable protocols after cycle detection"
            );
            let exclude: HashSet<&str> = cycle.path.iter().map(|s| s.as_str()).collect();
            loaded.graph.topological_order_excluding(&exclude)
        }
    };
    let topo_refs: Vec<&str> = topo_order.to_vec();

    let candidates = discover_ready_candidates(
        &loaded.manifest.protocols,
        &loaded.store,
        &topo_refs,
        &partially_scanned,
    );

    // 6. Select first viable candidate.
    let (candidate, protocol) = {
        let mut found = None;
        for c in &candidates {
            let Some(p) = loaded
                .manifest
                .protocols
                .iter()
                .find(|p| p.name == c.protocol_name)
            else {
                warn!(
                    operation = "candidate_selection",
                    outcome = "skipped_unknown_protocol",
                    protocol = %c.protocol_name,
                    work_unit = ?c.work_unit,
                    "skipping unknown protocol candidate"
                );
                continue;
            };
            let p = p.clone();

            if let Err(e) =
                handler::validate_output_types(&p, &loaded.store, c.work_unit.as_deref())
            {
                warn!(
                    operation = "candidate_selection",
                    outcome = "skipped_invalid_output_types",
                    protocol = %p.name,
                    work_unit = ?c.work_unit,
                    error = %e,
                    "skipping protocol candidate with unsupported output types"
                );
                continue;
            }
            found = Some((c, p));
            break;
        }
        match found {
            Some(pair) => pair,
            None => return Err("no viable protocol candidates found".into()),
        }
    };

    info!(
        operation = "mcp_session",
        outcome = "serving",
        protocol = %candidate.protocol_name,
        work_unit = ?candidate.work_unit,
        "serving protocol candidate"
    );

    // 7. Build handler.
    let handler = RunaHandler::new(
        protocol.clone(),
        candidate.work_unit.clone(),
        loaded.store,
        loaded.workspace_dir.clone(),
    );

    // 8. Serve via stdio transport.
    let (stdin, stdout) = io::stdio();
    let service = handler.serve((stdin, stdout)).await.inspect_err(|e| {
        error!(
            operation = "mcp_server",
            outcome = "init_failed",
            error = %e,
            "server initialization failed"
        );
    })?;
    service.waiting().await?;

    // 9. Re-scan workspace with a fresh store.
    let store_dir = runa_dir.join(STORE_DIRNAME);
    let mut store = ArtifactStore::new(loaded.manifest.artifact_types.clone(), store_dir)?;
    let post_scan = scan(&loaded.workspace_dir, &mut store)?;

    // 10. Check postconditions.
    let work_unit_ref = candidate.work_unit.as_deref();
    let output_type_names: HashSet<&str> = protocol
        .produces
        .iter()
        .chain(protocol.may_produce.iter())
        .map(|s| s.as_str())
        .collect();
    let partial_output_types: Vec<&str> = post_scan
        .partially_scanned_types
        .iter()
        .filter(|ps| output_type_names.contains(ps.artifact_type.as_str()))
        .map(|ps| ps.artifact_type.as_str())
        .collect();
    if !partial_output_types.is_empty() {
        warn!(
            operation = "postconditions",
            outcome = "scan_incomplete",
            protocol = %protocol.name,
            work_unit = ?candidate.work_unit,
            artifact_types = ?partial_output_types,
            "post-session scan incomplete for output types"
        );
        eprintln!(
            "runa-mcp: post-session scan incomplete for output types {partial_output_types:?} of '{}' work_unit={:?}",
            protocol.name, candidate.work_unit
        );
    } else if enforce_postconditions(&protocol, &store, work_unit_ref).is_ok() {
        info!(
            operation = "postconditions",
            outcome = "met",
            protocol = %protocol.name,
            work_unit = ?candidate.work_unit,
            "postconditions met"
        );
    } else {
        warn!(
            operation = "postconditions",
            outcome = "not_met",
            protocol = %protocol.name,
            work_unit = ?candidate.work_unit,
            "postconditions not met"
        );
        eprintln!(
            "runa-mcp: postconditions not met for '{}' work_unit={:?}",
            protocol.name, candidate.work_unit
        );
    }

    // 11. Exit 0.
    Ok(())
}
