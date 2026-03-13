mod commands;
mod project;

use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "runa", version)]
struct Cli {
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
    /// Display skills, dependencies, and execution order
    List,
    /// Check project health: validate artifacts, identify problems
    Doctor,
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init { methodology } => {
            let working_dir = match std::env::current_dir() {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("error: {e}");
                    process::exit(1);
                }
            };

            match commands::init::run(&working_dir, &methodology) {
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

            if let Err(e) = commands::list::run(&working_dir) {
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

            match commands::doctor::run(&working_dir) {
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
