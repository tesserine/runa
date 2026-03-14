mod commands;
mod project;

use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand};

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
    /// Display skills, dependencies, and execution order
    List,
    /// Check project health: validate artifacts, identify problems
    Doctor,
    /// Scan the artifact workspace and reconcile it into the store
    Scan,
    /// Manage operator-controlled runtime signals
    Signal {
        #[command(subcommand)]
        command: SignalCommand,
    },
    /// Evaluate skill readiness and report status
    Status {
        /// Emit machine-readable JSON instead of text output
        #[arg(long)]
        json: bool,
    },
    /// Build an execution plan for skills that are ready to run
    Step {
        /// Show the execution plan without attempting agent execution
        #[arg(long)]
        dry_run: bool,

        /// Emit machine-readable JSON instead of text output
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum SignalCommand {
    /// Ensure the named signal is active
    Begin {
        /// Signal name, must match [a-z][a-z0-9-]*
        name: String,
    },
    /// Ensure the named signal is inactive
    Clear {
        /// Signal name, must match [a-z][a-z0-9-]*
        name: String,
    },
    /// List active signals
    List,
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

    match cli.command {
        Commands::Init {
            methodology,
            artifacts_dir,
        } => {
            let working_dir = match std::env::current_dir() {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("error: {e}");
                    process::exit(1);
                }
            };

            match commands::init::run(
                &working_dir,
                &methodology,
                artifacts_dir.as_deref(),
                config_override.as_deref(),
            ) {
                Ok(summary) => {
                    println!(
                        "Initialized runa project with methodology '{}'",
                        summary.methodology_name
                    );
                    println!(
                        "  {} artifact types, {} skills",
                        summary.artifact_type_count, summary.skill_count
                    );
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    process::exit(1);
                }
            }
        }
        Commands::List => {
            let working_dir = match std::env::current_dir() {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("error: {e}");
                    process::exit(1);
                }
            };

            if let Err(e) = commands::list::run(&working_dir, config_override.as_deref()) {
                eprintln!("error: {e}");
                process::exit(1);
            }
        }
        Commands::Doctor => {
            let working_dir = match std::env::current_dir() {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("error: {e}");
                    process::exit(1);
                }
            };

            match commands::doctor::run(&working_dir, config_override.as_deref()) {
                Ok(healthy) => {
                    if !healthy {
                        process::exit(1);
                    }
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    process::exit(1);
                }
            }
        }
        Commands::Scan => {
            let working_dir = match std::env::current_dir() {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("error: {e}");
                    process::exit(1);
                }
            };

            if let Err(e) = commands::scan::run(&working_dir, config_override.as_deref()) {
                eprintln!("error: {e}");
                process::exit(1);
            }
        }
        Commands::Signal { command } => {
            let working_dir = match std::env::current_dir() {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("error: {e}");
                    process::exit(1);
                }
            };

            let result = match command {
                SignalCommand::Begin { name } => commands::signal::begin(&working_dir, &name),
                SignalCommand::Clear { name } => commands::signal::clear(&working_dir, &name),
                SignalCommand::List => commands::signal::list(&working_dir),
            };

            if let Err(e) = result {
                eprintln!("error: {e}");
                process::exit(1);
            }
        }
        Commands::Status { json } => {
            let working_dir = match std::env::current_dir() {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("error: {e}");
                    process::exit(1);
                }
            };

            if let Err(e) = commands::status::run(&working_dir, config_override.as_deref(), json) {
                eprintln!("error: {e}");
                process::exit(1);
            }
        }
        Commands::Step { dry_run, json } => {
            let working_dir = match std::env::current_dir() {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("error: {e}");
                    process::exit(1);
                }
            };

            if let Err(e) =
                commands::step::run(&working_dir, config_override.as_deref(), dry_run, json)
            {
                eprintln!("error: {e}");
                process::exit(1);
            }
        }
    }
}
