mod handler;

use clap::Parser;
use libagent::configure_tracing;
use libagent::project;
use rmcp::service::ServiceExt;
use rmcp::transport::io;
use std::io::Write;
use std::path::PathBuf;
use std::process;
use tracing::{error, info};

use handler::RunaHandler;

#[derive(Parser)]
#[command(name = "runa-mcp", version)]
struct Cli {
    /// Name of the protocol to serve
    #[arg(long)]
    protocol: String,

    /// Optional work unit scope for tool serving
    #[arg(long)]
    work_unit: Option<String>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let cli = Cli::parse();

    if let Err(err) = configure_tracing(None) {
        let _ = writeln!(std::io::stderr(), "runa-mcp: {err}");
        process::exit(1);
    }

    if let Err(e) = run(cli).await {
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

async fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let working_dir = match std::env::var("RUNA_WORKING_DIR") {
        Ok(dir) => PathBuf::from(dir),
        Err(_) => std::env::current_dir()?,
    };
    let config_override = std::env::var("RUNA_CONFIG").ok().map(PathBuf::from);
    let config_ref = config_override.as_deref();
    if let Ok(config) = project::read_config(&working_dir, config_ref) {
        configure_tracing(Some(&config.logging))?;
    }

    let loaded = project::load(&working_dir, config_ref)?;
    let protocol = loaded
        .manifest
        .protocols
        .iter()
        .find(|protocol| protocol.name == cli.protocol)
        .cloned()
        .ok_or_else(|| {
            format!(
                "protocol '{}' not found in manifest '{}'",
                cli.protocol, loaded.manifest.name
            )
        })?;

    handler::validate_protocol_scope(&protocol, cli.work_unit.as_deref()).map_err(|err| {
        format!(
            "protocol '{}' cannot be served via MCP tools: {err}",
            protocol.name
        )
    })?;

    handler::validate_output_types(&protocol, &loaded.store, cli.work_unit.as_deref()).map_err(
        |err| {
            format!(
                "protocol '{}' cannot be served via MCP tools: {err}",
                protocol.name
            )
        },
    )?;

    info!(
        operation = "mcp_session",
        outcome = "serving",
        protocol = %protocol.name,
        work_unit = ?cli.work_unit,
        "serving protocol"
    );

    let handler = RunaHandler::new(
        protocol.clone(),
        cli.work_unit.clone(),
        loaded.store,
        loaded.workspace_dir.clone(),
    );

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

    Ok(())
}
