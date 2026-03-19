mod context;
mod handler;

use std::collections::HashSet;
use std::path::PathBuf;
use std::process;
use std::sync::atomic::Ordering;

use libagent::project::{self, RUNA_DIR, STORE_DIRNAME};
use libagent::{
    ActivationStore, ArtifactStore, discover_ready_candidates, enforce_postconditions, scan,
};
use rmcp::service::ServiceExt;
use rmcp::transport::io;

use handler::RunaHandler;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(e) = run().await {
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

    // 2. Load project.
    let mut loaded = project::load(&working_dir, config_ref)?;
    let runa_dir = working_dir.join(RUNA_DIR);

    // 3. Scan workspace.
    let scan_result = scan(&loaded.workspace_dir, &mut loaded.store)?;

    // 4. Load signals.
    let (active_signals, signal_warnings) = project::load_signals(&runa_dir);
    for warning in &signal_warnings {
        eprintln!("warning: {warning}");
    }

    // 5. Load activation timestamps.
    let mut activations = ActivationStore::load(&runa_dir)?;

    // 6. Collect partially scanned types.
    let partially_scanned: HashSet<String> = scan_result
        .partially_scanned_types
        .iter()
        .map(|p| p.artifact_type.clone())
        .collect();

    // 7. Discover ready candidates.
    let topo_order = loaded.graph.topological_order()?;
    let topo_refs: Vec<&str> = topo_order.to_vec();

    let candidates = discover_ready_candidates(
        &loaded.manifest.protocols,
        &loaded.store,
        &activations,
        &active_signals,
        &topo_refs,
        &partially_scanned,
    );

    // 8. Select first candidate.
    let candidate = match candidates.first() {
        Some(c) => c,
        None => {
            eprintln!("runa-mcp: no ready (protocol, work_unit) candidates found");
            process::exit(1);
        }
    };

    let protocol = loaded
        .manifest
        .protocols
        .iter()
        .find(|p| p.name == candidate.protocol_name)
        .expect("candidate protocol exists in manifest")
        .clone();

    eprintln!(
        "runa-mcp: serving protocol '{}' work_unit={:?}",
        candidate.protocol_name, candidate.work_unit
    );

    // 9. Build handler.
    let handler = RunaHandler::new(
        protocol.clone(),
        candidate.work_unit.clone(),
        loaded.store,
        loaded.manifest.clone(),
        loaded.workspace_dir.clone(),
    );

    // 10. Serve via stdio transport.
    let output_produced = handler.output_produced();
    let (stdin, stdout) = io::stdio();
    let service = handler
        .serve((stdin, stdout))
        .await
        .inspect_err(|e| eprintln!("runa-mcp: server init failed: {e}"))?;
    service.waiting().await?;

    // 11. Re-scan workspace with a fresh store.
    let store_dir = runa_dir.join(STORE_DIRNAME);
    let mut store = ArtifactStore::new(loaded.manifest.artifact_types.clone(), store_dir)?;
    scan(&loaded.workspace_dir, &mut store)?;

    // 12. Check whether the session produced output and postconditions pass.
    let work_unit_ref = candidate.work_unit.as_deref();
    if !output_produced.load(Ordering::Relaxed) {
        eprintln!(
            "runa-mcp: no output produced for '{}' work_unit={:?}, no activation recorded",
            protocol.name, candidate.work_unit
        );
    } else if enforce_postconditions(&protocol, &store, work_unit_ref).is_ok() {
        // 13. Record activation.
        activations.record(&protocol.name, work_unit_ref);
        activations.save(&runa_dir)?;
        eprintln!(
            "runa-mcp: postconditions met, activation recorded for '{}' work_unit={:?}",
            protocol.name, candidate.work_unit
        );
    } else {
        eprintln!(
            "runa-mcp: postconditions not met for '{}' work_unit={:?}, no activation recorded",
            protocol.name, candidate.work_unit
        );
    }

    // 14. Exit 0.
    Ok(())
}
