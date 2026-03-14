use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use libagent::{ArtifactStore, DependencyGraph, GraphError, Manifest, ManifestError, StoreError};

pub const RUNA_DIR: &str = ".runa";
pub const CONFIG_FILENAME: &str = "config.toml";
pub const STATE_FILENAME: &str = "state.toml";
pub const SIGNALS_FILENAME: &str = "signals.json";
pub(crate) const DEFAULT_WORKSPACE_DIR: &str = "workspace";
pub(crate) const STORE_DIRNAME: &str = "store";

/// On-disk format for `.runa/config.toml` — operator configuration.
#[derive(Serialize, Deserialize)]
pub struct Config {
    pub methodology_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts_dir: Option<String>,
}

/// On-disk format for `.runa/state.toml` — initialization metadata managed by runa.
#[derive(Serialize, Deserialize)]
pub struct State {
    pub initialized_at: String,
    pub runa_version: String,
}

/// A fully loaded runa project: manifest, dependency graph, artifact store, and active signals.
pub struct LoadedProject {
    pub manifest: Manifest,
    pub graph: DependencyGraph,
    pub store: ArtifactStore,
    pub workspace_dir: PathBuf,
    pub active_signals: std::collections::HashSet<String>,
}

/// Errors that can occur when loading a runa project.
#[derive(Debug)]
pub enum ProjectError {
    /// No config file found in the resolution chain.
    ConfigNotFound,
    /// An explicitly provided config path does not exist.
    ConfigPathNotFound(PathBuf),
    /// Config file exists but cannot be parsed.
    ConfigParseFailed(String),
    /// `.runa/state.toml` is missing — project not initialized.
    NotInitialized,
    /// Filesystem I/O failure.
    Io(std::io::Error),
    /// `state.toml` exists but cannot be parsed.
    StateParseFailed(String),
    /// `signals.json` exists but cannot be parsed.
    SignalsParseFailed(String),
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
            ProjectError::ConfigNotFound => {
                write!(f, "no config found (run 'runa init' first)")
            }
            ProjectError::ConfigPathNotFound(path) => {
                write!(f, "config not found: {}", path.display())
            }
            ProjectError::ConfigParseFailed(detail) => {
                write!(f, "failed to parse config: {detail}")
            }
            ProjectError::NotInitialized => {
                write!(f, "not a runa project (run 'runa init' first)")
            }
            ProjectError::Io(e) => write!(f, "{e}"),
            ProjectError::StateParseFailed(detail) => {
                write!(f, "failed to parse .runa/state.toml: {detail}")
            }
            ProjectError::SignalsParseFailed(detail) => {
                write!(f, "failed to parse .runa/signals.json: {detail}")
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

/// Resolve the config file path using the resolution chain.
///
/// The chain returns the first config file found and uses it entirely.
/// There is no per-field merging across config files — this is intentional.
/// Merging adds complexity for a use case that doesn't exist.
///
/// Resolution order:
/// 1. Explicit override (from `--config` CLI flag or `RUNA_CONFIG` env var)
/// 2. `.runa/config.toml` in working directory
/// 3. `$XDG_CONFIG_HOME/runa/config.toml` (falls back to `$HOME/.config/runa/config.toml`)
pub(crate) fn resolve_config(
    working_dir: &Path,
    config_override: Option<&Path>,
) -> Result<PathBuf, ProjectError> {
    // 1. Explicit override — caller already resolved --config vs RUNA_CONFIG.
    if let Some(path) = config_override {
        return if path.exists() {
            Ok(path.to_path_buf())
        } else {
            Err(ProjectError::ConfigPathNotFound(path.to_path_buf()))
        };
    }

    // 2. Project-level config.
    let project_config = working_dir.join(RUNA_DIR).join(CONFIG_FILENAME);
    if project_config.exists() {
        return Ok(project_config);
    }

    // 3. XDG config.
    let xdg_base = match std::env::var("XDG_CONFIG_HOME") {
        Ok(val) if !val.is_empty() => PathBuf::from(val),
        _ => {
            let home = std::env::var("HOME").unwrap_or_default();
            if home.is_empty() {
                return Err(ProjectError::ConfigNotFound);
            }
            PathBuf::from(home).join(".config")
        }
    };
    let xdg_config = xdg_base.join("runa").join(CONFIG_FILENAME);
    if xdg_config.exists() {
        return Ok(xdg_config);
    }

    Err(ProjectError::ConfigNotFound)
}

/// Load a runa project from `working_dir`.
///
/// Resolves config via the resolution chain, reads state, parses the methodology
/// manifest, builds the dependency graph, opens the artifact store, and loads
/// active signals from `.runa/signals.json` if present.
pub fn load(
    working_dir: &Path,
    config_override: Option<&Path>,
) -> Result<LoadedProject, ProjectError> {
    let config_path = resolve_config(working_dir, config_override)?;

    let config_content = std::fs::read_to_string(&config_path).map_err(ProjectError::Io)?;
    let config: Config = toml::from_str(&config_content)
        .map_err(|e| ProjectError::ConfigParseFailed(e.to_string()))?;

    // Verify state.toml exists (project was initialized).
    let runa_dir = working_dir.join(RUNA_DIR);
    let state_path = runa_dir.join(STATE_FILENAME);
    let state_content = match std::fs::read_to_string(&state_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ProjectError::NotInitialized);
        }
        Err(e) => return Err(ProjectError::Io(e)),
    };
    let _state: State = toml::from_str(&state_content)
        .map_err(|e| ProjectError::StateParseFailed(e.to_string()))?;

    let manifest = libagent::manifest::parse(Path::new(&config.methodology_path))
        .map_err(ProjectError::ManifestInvalid)?;

    let graph = DependencyGraph::build(&manifest.skills).map_err(ProjectError::GraphInvalid)?;
    let active_signals = load_signals(&runa_dir)?;

    // Resolve artifact workspace dir: explicit config value or default,
    // relative to the project `.runa/` directory.
    let workspace_dir = match &config.artifacts_dir {
        Some(dir) => working_dir.join(dir),
        None => runa_dir.join(DEFAULT_WORKSPACE_DIR),
    };
    let store_dir = runa_dir.join(STORE_DIRNAME);
    let store = ArtifactStore::new(manifest.artifact_types.clone(), store_dir)
        .map_err(ProjectError::StoreError)?;

    Ok(LoadedProject {
        manifest,
        graph,
        store,
        workspace_dir,
        active_signals,
    })
}

#[derive(Deserialize)]
struct SignalsFile {
    active: Vec<String>,
}

pub(crate) fn load_signals(
    runa_dir: &Path,
) -> Result<std::collections::HashSet<String>, ProjectError> {
    let path = runa_dir.join(SIGNALS_FILENAME);
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(std::collections::HashSet::new());
        }
        Err(err) => return Err(ProjectError::Io(err)),
    };

    let parsed: SignalsFile = serde_json::from_str(&content)
        .map_err(|err| ProjectError::SignalsParseFailed(err.to_string()))?;
    Ok(parsed.active.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // LoadedProject can't derive Debug (ArtifactStore doesn't impl it),
    // but unwrap_err() requires T: Debug. Provide a minimal impl for tests.
    impl std::fmt::Debug for LoadedProject {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("LoadedProject").finish_non_exhaustive()
        }
    }

    fn valid_manifest_toml() -> &'static str {
        r#"
name = "test-methodology"

[[artifact_types]]
name = "constraints"
schema = { type = "object" }

[[skills]]
name = "ground"
produces = ["constraints"]
trigger = { type = "on_signal", name = "init" }
"#
    }

    fn write_project_files(working: &Path, manifest_path: &Path) {
        let runa_dir = working.join(".runa");
        fs::create_dir_all(&runa_dir).unwrap();

        let canonical = fs::canonicalize(manifest_path).unwrap();
        let config = Config {
            methodology_path: canonical.display().to_string(),
            artifacts_dir: None,
        };
        fs::write(
            runa_dir.join("config.toml"),
            toml::to_string(&config).unwrap(),
        )
        .unwrap();

        let state = State {
            initialized_at: "2026-01-01T00:00:00Z".to_string(),
            runa_version: "0.1.0".to_string(),
        };
        fs::write(
            runa_dir.join("state.toml"),
            toml::to_string(&state).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn load_reads_config_and_state() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("manifest.toml");
        fs::write(&manifest_path, valid_manifest_toml()).unwrap();

        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();
        write_project_files(&working, &manifest_path);

        let loaded = load(&working, None).unwrap();
        assert_eq!(loaded.manifest.name, "test-methodology");
        assert_eq!(loaded.workspace_dir, working.join(".runa/workspace"));
        assert!(loaded.active_signals.is_empty());
    }

    #[test]
    fn load_reads_active_signals_from_signals_file() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("manifest.toml");
        fs::write(&manifest_path, valid_manifest_toml()).unwrap();

        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();
        write_project_files(&working, &manifest_path);
        fs::write(
            working.join(".runa").join("signals.json"),
            r#"{ "active": ["deploy", "begin"] }"#,
        )
        .unwrap();

        let loaded = load(&working, None).unwrap();
        assert_eq!(loaded.active_signals.len(), 2);
        assert!(loaded.active_signals.contains("deploy"));
        assert!(loaded.active_signals.contains("begin"));
    }

    #[test]
    fn load_treats_missing_signals_file_as_empty_set() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("manifest.toml");
        fs::write(&manifest_path, valid_manifest_toml()).unwrap();

        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();
        write_project_files(&working, &manifest_path);

        let signals = load_signals(&working.join(".runa")).unwrap();
        assert!(signals.is_empty());
    }

    #[test]
    fn load_fails_when_signals_file_is_malformed() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("manifest.toml");
        fs::write(&manifest_path, valid_manifest_toml()).unwrap();

        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();
        write_project_files(&working, &manifest_path);
        fs::write(working.join(".runa").join("signals.json"), "{not json").unwrap();

        let err = load(&working, None).unwrap_err();
        assert!(
            matches!(err, ProjectError::SignalsParseFailed(_)),
            "expected SignalsParseFailed, got: {err}"
        );
    }

    #[test]
    fn load_with_explicit_config_override() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("manifest.toml");
        fs::write(&manifest_path, valid_manifest_toml()).unwrap();

        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();

        // Write state.toml in the project but config elsewhere.
        let runa_dir = working.join(".runa");
        fs::create_dir_all(&runa_dir).unwrap();
        let state = State {
            initialized_at: "2026-01-01T00:00:00Z".to_string(),
            runa_version: "0.1.0".to_string(),
        };
        fs::write(
            runa_dir.join("state.toml"),
            toml::to_string(&state).unwrap(),
        )
        .unwrap();

        let canonical = fs::canonicalize(&manifest_path).unwrap();
        let external_config_path = dir.path().join("external-config.toml");
        let config = Config {
            methodology_path: canonical.display().to_string(),
            artifacts_dir: None,
        };
        fs::write(&external_config_path, toml::to_string(&config).unwrap()).unwrap();

        let loaded = load(&working, Some(&external_config_path)).unwrap();
        assert_eq!(loaded.manifest.name, "test-methodology");
    }

    #[test]
    fn load_with_custom_artifacts_dir() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("manifest.toml");
        fs::write(&manifest_path, valid_manifest_toml()).unwrap();

        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();

        let runa_dir = working.join(".runa");
        fs::create_dir_all(&runa_dir).unwrap();

        let canonical = fs::canonicalize(&manifest_path).unwrap();
        let config = Config {
            methodology_path: canonical.display().to_string(),
            artifacts_dir: Some("custom-artifacts".to_string()),
        };
        fs::write(
            runa_dir.join("config.toml"),
            toml::to_string(&config).unwrap(),
        )
        .unwrap();

        let state = State {
            initialized_at: "2026-01-01T00:00:00Z".to_string(),
            runa_version: "0.1.0".to_string(),
        };
        fs::write(
            runa_dir.join("state.toml"),
            toml::to_string(&state).unwrap(),
        )
        .unwrap();

        // load succeeds — artifacts_dir is resolved but doesn't need to exist yet
        let loaded = load(&working, None).unwrap();
        assert_eq!(loaded.workspace_dir, working.join("custom-artifacts"));
    }

    #[test]
    fn missing_config_returns_config_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let err = load(dir.path(), None).unwrap_err();
        assert!(
            matches!(err, ProjectError::ConfigNotFound),
            "expected ConfigNotFound, got: {err}"
        );
    }

    #[test]
    fn missing_state_returns_not_initialized() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("manifest.toml");
        fs::write(&manifest_path, valid_manifest_toml()).unwrap();

        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();

        // Write config but no state.
        let runa_dir = working.join(".runa");
        fs::create_dir_all(&runa_dir).unwrap();
        let canonical = fs::canonicalize(&manifest_path).unwrap();
        let config = Config {
            methodology_path: canonical.display().to_string(),
            artifacts_dir: None,
        };
        fs::write(
            runa_dir.join("config.toml"),
            toml::to_string(&config).unwrap(),
        )
        .unwrap();

        let err = load(&working, None).unwrap_err();
        assert!(
            matches!(err, ProjectError::NotInitialized),
            "expected NotInitialized, got: {err}"
        );
    }

    #[test]
    fn resolve_config_prefers_override() {
        let dir = tempfile::tempdir().unwrap();
        let override_path = dir.path().join("override.toml");
        fs::write(&override_path, "methodology_path = \"x\"").unwrap();

        // Also create project-level config to prove override wins.
        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();
        let runa_dir = working.join(".runa");
        fs::create_dir_all(&runa_dir).unwrap();
        fs::write(runa_dir.join("config.toml"), "methodology_path = \"y\"").unwrap();

        let resolved = resolve_config(&working, Some(&override_path)).unwrap();
        assert_eq!(resolved, override_path);
    }

    #[test]
    fn resolve_config_falls_back_to_project_level() {
        let dir = tempfile::tempdir().unwrap();
        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();
        let runa_dir = working.join(".runa");
        fs::create_dir_all(&runa_dir).unwrap();
        fs::write(runa_dir.join("config.toml"), "methodology_path = \"x\"").unwrap();

        let resolved = resolve_config(&working, None).unwrap();
        assert_eq!(resolved, runa_dir.join("config.toml"));
    }

    #[test]
    fn resolve_config_returns_error_when_none_found() {
        let dir = tempfile::tempdir().unwrap();
        let err = resolve_config(dir.path(), None).unwrap_err();
        assert!(
            matches!(err, ProjectError::ConfigNotFound),
            "expected ConfigNotFound, got: {err}"
        );
    }

    #[test]
    fn resolve_config_explicit_path_not_found_returns_distinct_error() {
        let dir = tempfile::tempdir().unwrap();
        let bogus = dir.path().join("nonexistent.toml");
        let err = resolve_config(dir.path(), Some(&bogus)).unwrap_err();
        assert!(
            matches!(err, ProjectError::ConfigPathNotFound(_)),
            "expected ConfigPathNotFound, got: {err}"
        );
        assert!(
            err.to_string().contains("nonexistent.toml"),
            "error should include the path"
        );
    }
}
