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
    #[arg(long, conflicts_with = "session", required_unless_present = "session")]
    protocol: Option<String>,

    /// Serve a scoped session surface instead of one fixed protocol
    #[arg(long)]
    session: bool,

    /// Optional work unit scope for tool serving
    #[arg(long)]
    work_unit: Option<String>,

    /// Open a session from a forge ticket reference (cold-start entry)
    #[arg(long, requires = "session", conflicts_with_all = ["work_unit", "protocol"])]
    ticket: Option<String>,
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

    let mut loaded = project::load(&working_dir, config_ref)?;
    apply_transcript_settings(&working_dir, &loaded.config);
    libagent::scan(&loaded.workspace_dir, &mut loaded.store)?;
    let identity = libagent::resolve_forge_identity(&loaded.config.forge);
    if let Some(work_unit) = cli.work_unit.as_deref() {
        libagent::validate_scoped_work_unit_with_identity(&loaded.store, work_unit, &identity)?;
    }
    if cli.session {
        let handler = match cli.ticket.as_deref() {
            Some(ticket) => {
                let ticket_ref = libagent::resolve_ticket_reference(ticket, &identity)?;
                RunaHandler::new_session_entry(
                    working_dir.clone(),
                    config_ref,
                    ticket_ref,
                    identity.clone(),
                )?
            }
            None => RunaHandler::new_session(
                working_dir.clone(),
                config_ref,
                cli.work_unit,
                identity.clone(),
            )?,
        };
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
        return Ok(());
    }

    let protocol_name = cli.protocol.expect("--protocol required without --session");
    let protocol = loaded
        .manifest
        .protocols
        .iter()
        .find(|protocol| protocol.name == protocol_name)
        .cloned()
        .ok_or_else(|| {
            format!(
                "protocol '{}' not found in manifest '{}'",
                protocol_name, loaded.manifest.name
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

    let forge_runtime =
        runa_forge_compose::runtime_from_config_with_identity(&loaded.config.forge, &identity)
            .map_err(|error| format!("failed to compose forge connector tools: {error}"))?;

    let handler = RunaHandler::new(
        protocol.clone(),
        cli.work_unit.clone(),
        loaded.store,
        loaded.workspace_dir.clone(),
        forge_runtime,
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

fn apply_transcript_settings(working_dir: &std::path::Path, config: &project::Config) {
    let settings = libagent::transcript::resolve_transcript_settings_with_forge(
        working_dir,
        &config.transcript,
        &config.forge,
    );
    for (name, value) in libagent::transcript::transcript_env_from_settings(&settings) {
        unsafe { std::env::set_var(name, value) };
    }
}
