use std::fmt;
use std::path::Path;

use libagent::ScanResult;

use crate::project::{self, LoadedProject, ProjectError};

pub mod doctor;
pub mod init;
pub mod list;
pub mod protocol_eval;
pub mod run;
pub mod scan;
pub mod state;
pub mod step;

#[derive(Debug)]
pub enum CommandError {
    Project(ProjectError),
    Scan(libagent::ScanError),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommandError::Project(err) => write!(f, "{err}"),
            CommandError::Scan(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for CommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CommandError::Project(err) => Some(err),
            CommandError::Scan(err) => Some(err),
        }
    }
}

impl From<ProjectError> for CommandError {
    fn from(err: ProjectError) -> Self {
        CommandError::Project(err)
    }
}

impl From<libagent::ScanError> for CommandError {
    fn from(err: libagent::ScanError) -> Self {
        CommandError::Scan(err)
    }
}

pub fn load_and_scan(
    working_dir: &Path,
    config_override: Option<&Path>,
) -> Result<(LoadedProject, ScanResult), CommandError> {
    let mut loaded = project::load(working_dir, config_override)?;
    let scan_result = libagent::scan(&loaded.workspace_dir, &mut loaded.store)?;
    Ok((loaded, scan_result))
}
