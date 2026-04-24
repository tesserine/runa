mod commands;
mod exit_codes;
mod project;

use std::path::PathBuf;
use std::process;
use std::{io, io::Write};

use clap::{Args, Parser, Subcommand};
use tracing::error;

use crate::exit_codes::ExitCode;

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

        /// Evaluate only the specified delegated work unit
        #[arg(long)]
        work_unit: Option<String>,
    },
    /// Build an execution plan for protocols that are ready to run
    Step {
        /// Show the execution plan without attempting agent execution
        #[arg(long)]
        dry_run: bool,

        /// Emit machine-readable JSON instead of text output
        #[arg(long)]
        json: bool,

        /// Evaluate only the specified delegated work unit
        #[arg(long)]
        work_unit: Option<String>,
    },
    /// Cascade through ready protocols until quiescence
    Run(RunArgs),
}

#[derive(Args)]
struct RunArgs {
    /// Show the projected cascade without attempting agent execution
    #[arg(long)]
    dry_run: bool,

    /// Emit machine-readable JSON instead of text output
    #[arg(long)]
    json: bool,

    /// Evaluate only the specified delegated work unit
    #[arg(long)]
    work_unit: Option<String>,

    /// Override the live agent command; pass argv after `--`, for example `--agent-command -- <argv tokens>`
    #[arg(long = "agent-command")]
    agent_command: bool,

    /// Agent argv passed through when `--agent-command` is set
    #[arg(
        num_args = 0..,
        trailing_var_arg = true,
        allow_hyphen_values = true,
        requires = "agent_command",
        value_name = "ARGV"
    )]
    agent_command_argv: Vec<String>,
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
        process::exit(ExitCode::InfrastructureFailure.code());
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
            process::exit(ExitCode::InfrastructureFailure.code());
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
        process::exit(ExitCode::InfrastructureFailure.code());
    }

    match cli.command {
        Commands::Init { methodology } => {
            match commands::init::run(&working_dir, &methodology, config_override_ref) {
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
                Err(err) => fatal_command_error("init", &err, ExitCode::InfrastructureFailure),
            }
        }
        Commands::List => {
            if let Err(err) = commands::list::run(&working_dir, config_override_ref) {
                fatal_command_error("list", &err, ExitCode::InfrastructureFailure);
            }
        }
        Commands::Doctor => match commands::doctor::run(&working_dir, config_override_ref) {
            Ok(healthy) => {
                if !healthy {
                    process::exit(ExitCode::GenericFailure.code());
                }
            }
            Err(err) => fatal_command_error("doctor", &err, ExitCode::InfrastructureFailure),
        },
        Commands::Scan => {
            if let Err(err) = commands::scan::run(&working_dir, config_override_ref) {
                fatal_command_error("scan", &err, ExitCode::InfrastructureFailure);
            }
        }
        Commands::State { json, work_unit } => {
            if let Err(err) = commands::state::run(
                &working_dir,
                config_override_ref,
                json,
                work_unit.as_deref(),
            ) {
                fatal_command_error("state", &err, ExitCode::InfrastructureFailure);
            }
        }
        Commands::Step {
            dry_run,
            json,
            work_unit,
        } => {
            match commands::step::run(
                &working_dir,
                config_override_ref,
                dry_run,
                json,
                work_unit.as_deref(),
            ) {
                Ok(outcome) => {
                    let exit_code = outcome.exit_code().code();
                    if exit_code != 0 {
                        process::exit(exit_code);
                    }
                }
                Err(err) => fatal_command_error("step", &err, err.exit_code()),
            }
        }
        Commands::Run(args) => {
            match commands::run::run(
                &working_dir,
                config_override_ref,
                args.dry_run,
                args.json,
                args.work_unit.as_deref(),
                args.agent_command,
                &args.agent_command_argv,
            ) {
                Ok(outcome) => {
                    let exit_code = outcome.exit_code();
                    if exit_code != 0 {
                        process::exit(exit_code);
                    }
                }
                Err(err) => fatal_command_error("run", &err, err.exit_code()),
            }
        }
    }
}

fn fatal_command_error(command: &str, err: &dyn std::fmt::Display, exit_code: ExitCode) -> ! {
    error!(
        operation = "command",
        command = command,
        outcome = "failed",
        error = %err,
        "command failed"
    );
    eprintln!("error: {err}");
    process::exit(exit_code.code())
}
