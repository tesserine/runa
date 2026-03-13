use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use libagent::{ArtifactStore, DependencyGraph, GraphError, Manifest, ManifestError, StoreError};

const RUNA_DIR: &str = ".runa";
const STATE_FILENAME: &str = "state.toml";
const ARTIFACTS_DIR: &str = "artifacts";

/// On-disk format for `.runa/state.toml`.
#[derive(Serialize, Deserialize)]
pub struct State {
    pub methodology_path: String,
    pub methodology_name: String,
}

/// A fully loaded runa project: manifest, dependency graph, and artifact store.
pub struct LoadedProject {
    pub manifest: Manifest,
    pub graph: DependencyGraph,
    pub store: ArtifactStore,
}

/// Errors that can occur when loading a runa project.
#[derive(Debug)]
pub enum ProjectError {
    /// `.runa/state.toml` is missing — not an initialized project.
    NotInitialized,
    /// Filesystem I/O failure.
    Io(std::io::Error),
    /// `state.toml` exists but cannot be parsed.
    StateParseFailed(String),
    /// The methodology manifest is invalid.
    ManifestInvalid(ManifestError),
    /// The dependency graph could not be built.
    GraphInvalid(GraphError),
    /// The artifact store could not be loaded.
    StoreError(StoreError),
}

impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProjectError::NotInitialized => {
                write!(f, "not a runa project (run 'runa init' first)")
            }
            ProjectError::Io(e) => write!(f, "{e}"),
            ProjectError::StateParseFailed(detail) => {
                write!(f, "failed to parse .runa/state.toml: {detail}")
            }
            ProjectError::ManifestInvalid(e) => write!(f, "{e}"),
            ProjectError::GraphInvalid(e) => write!(f, "{e}"),
            ProjectError::StoreError(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ProjectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ProjectError::Io(e) => Some(e),
            ProjectError::ManifestInvalid(e) => Some(e),
            ProjectError::GraphInvalid(e) => Some(e),
            ProjectError::StoreError(e) => Some(e),
            _ => None,
        }
    }
}

/// Load a runa project from `working_dir`.
///
/// Reads `.runa/state.toml`, parses the methodology manifest it references,
/// builds the dependency graph, and opens the artifact store.
pub fn load(working_dir: &Path) -> Result<LoadedProject, ProjectError> {
    let runa_dir = working_dir.join(RUNA_DIR);
    let state_path = runa_dir.join(STATE_FILENAME);

    let state_content = match std::fs::read_to_string(&state_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ProjectError::NotInitialized);
        }
        Err(e) => return Err(ProjectError::Io(e)),
    };

    let state: State = toml::from_str(&state_content)
        .map_err(|e| ProjectError::StateParseFailed(e.to_string()))?;

    let manifest = libagent::manifest::parse(Path::new(&state.methodology_path))
        .map_err(ProjectError::ManifestInvalid)?;

    let graph = DependencyGraph::build(&manifest.skills).map_err(ProjectError::GraphInvalid)?;

    let store_dir = runa_dir.join(ARTIFACTS_DIR);
    let store = ArtifactStore::new(manifest.artifact_types.clone(), store_dir)
        .map_err(ProjectError::StoreError)?;

    Ok(LoadedProject {
        manifest,
        graph,
        store,
    })
}
