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

        /// Directory for artifact storage (default: .runa/artifacts/)
        #[arg(long)]
        artifacts_dir: Option<String>,
    },
    /// Display skills, dependencies, and execution order
    List,
    /// Check project health: validate artifacts, identify problems
    Doctor,
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
    }
}
