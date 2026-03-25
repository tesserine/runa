mod commands;
mod project;

use std::path::PathBuf;
use std::process;
use std::{io, io::Write};

use clap::{Parser, Subcommand};
use tracing::error;

#[derive(Parser)]
#[command(name = "runa", version)]
struct Cli {
    /// Path to config file (overrides RUNA_CONFIG and default locations)
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a runa project in the current directory
    Init {
        /// Path to the methodology manifest file
        #[arg(long)]
        methodology: PathBuf,

        /// Directory for artifact workspace files (default: .runa/workspace/)
        #[arg(long)]
        artifacts_dir: Option<String>,
    },
    /// Display protocols, dependencies, and execution order
    List,
    /// Check project health: validate artifacts, identify problems
    Doctor,
    /// Scan the artifact workspace and reconcile it into the store
    Scan,
    /// Evaluate protocol readiness and report state
    State {
        /// Emit machine-readable JSON instead of text output
        #[arg(long)]
        json: bool,
    },
    /// Build an execution plan for protocols that are ready to run
    Step {
        /// Show the execution plan without attempting agent execution
        #[arg(long)]
        dry_run: bool,

        /// Emit machine-readable JSON instead of text output
        #[arg(long)]
        json: bool,
    },
}

fn main() {
    let cli = Cli::parse();

    // Resolve config override: --config flag takes precedence over RUNA_CONFIG env var.
    let config_override: Option<PathBuf> = cli.config.or_else(|| {
        std::env::var("RUNA_CONFIG")
            .ok()
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    });
    let config_override_ref = config_override.as_deref();

    if let Err(err) = libagent::configure_tracing(None) {
        let _ = writeln!(io::stderr(), "error: {err}");
        process::exit(1);
    }

    let working_dir = match std::env::current_dir() {
        Ok(d) => d,
        Err(err) => {
            error!(
                operation = "current_dir",
                outcome = "failed",
                error = %err,
                "failed to resolve working directory"
            );
            eprintln!("error: {err}");
            process::exit(1);
        }
    };

    if !matches!(cli.command, Commands::Init { .. })
        && let Ok(config) = project::read_config(&working_dir, config_override_ref)
        && let Err(err) = libagent::configure_tracing(Some(&config.logging))
    {
        error!(
            operation = "logging_config",
            outcome = "failed",
            error = %err,
            "failed to apply configured logging"
        );
        eprintln!("error: {err}");
        process::exit(1);
    }

    match cli.command {
        Commands::Init {
            methodology,
            artifacts_dir,
        } => {
            match commands::init::run(
                &working_dir,
                &methodology,
                artifacts_dir.as_deref(),
                config_override_ref,
            ) {
                Ok(summary) => {
                    println!(
                        "Initialized runa project with methodology '{}'",
                        summary.methodology_name
                    );
                    println!(
                        "  {} artifact types, {} protocols",
                        summary.artifact_type_count, summary.protocol_count
                    );
                }
                Err(err) => fatal_command_error("init", &err),
            }
        }
        Commands::List => {
            if let Err(err) = commands::list::run(&working_dir, config_override_ref) {
                fatal_command_error("list", &err);
            }
        }
        Commands::Doctor => match commands::doctor::run(&working_dir, config_override_ref) {
            Ok(healthy) => {
                if !healthy {
                    process::exit(1);
                }
            }
            Err(err) => fatal_command_error("doctor", &err),
        },
        Commands::Scan => {
            if let Err(err) = commands::scan::run(&working_dir, config_override_ref) {
                fatal_command_error("scan", &err);
            }
        }
        Commands::State { json } => {
            if let Err(err) = commands::state::run(&working_dir, config_override_ref, json) {
                fatal_command_error("state", &err);
            }
        }
        Commands::Step { dry_run, json } => {
            if let Err(err) = commands::step::run(&working_dir, config_override_ref, dry_run, json)
            {
                fatal_command_error("step", &err);
            }
        }
    }
}

fn fatal_command_error(command: &str, err: &dyn std::fmt::Display) -> ! {
    error!(
        operation = "command",
        command = command,
        outcome = "failed",
        error = %err,
        "command failed"
    );
    eprintln!("error: {err}");
    process::exit(1)
}
