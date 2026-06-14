//! Shared project loading logic used by both `runa-cli` and `runa-mcp`.
//!
//! Resolves the config file via the resolution chain, parses the methodology
//! manifest, builds the dependency graph, and initializes the artifact store.
//! See [`load`] for the main entry point.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{ArtifactStore, DependencyGraph, GraphError, Manifest, ManifestError, StoreError};

pub const RUNA_DIR: &str = ".runa";
pub const CONFIG_FILENAME: &str = "config.toml";
pub const PROJECT_FILENAME: &str = "project.toml";
pub const STATE_FILENAME: &str = "state.toml";
pub const DEFAULT_WORKSPACE_DIR: &str = "workspace";
pub const STORE_DIRNAME: &str = "store";
pub const RUNA_TARGET_PROJECT: &str = "RUNA_TARGET_PROJECT";

/// Output format for tracing events written to stderr.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LogFormat {
    #[default]
    Text,
    Json,
}

/// The `[logging]` section of `.runa/config.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LoggingConfig {
    #[serde(default)]
    pub format: LogFormat,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
}

impl LoggingConfig {
    fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

/// The `[launch]` section of `.runa/project.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LaunchConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
}

impl LaunchConfig {
    fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

/// The `[transcript]` section of `.runa/config.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TranscriptConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redact_env: Vec<String>,
}

impl TranscriptConfig {
    fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ForgeType {
    #[default]
    Github,
    Sourcehut,
}

impl ForgeType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Sourcehut => "sourcehut",
        }
    }

    pub fn parse(value: &str) -> Result<Self, UnsupportedForgeError> {
        match value {
            "github" => Ok(Self::Github),
            "sourcehut" => Ok(Self::Sourcehut),
            other => Err(UnsupportedForgeError {
                forge_type: other.to_string(),
            }),
        }
    }
}

impl fmt::Display for ForgeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedForgeError {
    pub forge_type: String,
}

impl fmt::Display for UnsupportedForgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unsupported forge type `{}`", self.forge_type)
    }
}

impl std::error::Error for UnsupportedForgeError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryConfig {
    pub selector: String,
    pub host: String,
    pub owner: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackerConfig {
    pub selector: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracker_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TargetProjectConfig {
    #[serde(default)]
    pub forge_type: ForgeType,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repositories: Vec<RepositoryConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trackers: Vec<TrackerConfig>,
}

impl TargetProjectConfig {
    fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

/// Structured child-process payload delivered through `RUNA_TARGET_PROJECT`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TargetProjectPayload {
    pub version: u32,
    pub forge_type: ForgeType,
    pub repositories: Vec<RepositoryConfig>,
    pub trackers: Vec<TrackerConfig>,
}

impl From<&TargetProjectConfig> for TargetProjectPayload {
    fn from(value: &TargetProjectConfig) -> Self {
        Self {
            version: 1,
            forge_type: value.forge_type,
            repositories: value.repositories.clone(),
            trackers: value.trackers.clone(),
        }
    }
}

/// Effective runtime configuration after combining machine-local config and
/// portable project config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub methodology_path: String,
    #[serde(default, skip_serializing_if = "LoggingConfig::is_default")]
    pub logging: LoggingConfig,
    #[serde(default, skip_serializing_if = "LaunchConfig::is_default")]
    pub launch: LaunchConfig,
    #[serde(default, skip_serializing_if = "TranscriptConfig::is_default")]
    pub transcript: TranscriptConfig,
    #[serde(default, skip_serializing_if = "TargetProjectConfig::is_default")]
    pub target_project: TargetProjectConfig,
}

/// On-disk format for machine-local `.runa/config.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineConfig {
    pub methodology_path: String,
    #[serde(default, skip_serializing_if = "LoggingConfig::is_default")]
    pub logging: LoggingConfig,
    #[serde(default, skip_serializing_if = "TranscriptConfig::is_default")]
    pub transcript: TranscriptConfig,
}

/// On-disk format for portable `.runa/project.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProjectConfig {
    #[serde(default, skip_serializing_if = "LaunchConfig::is_default")]
    pub launch: LaunchConfig,
    #[serde(default, skip_serializing_if = "TranscriptConfig::is_default")]
    pub transcript: TranscriptConfig,
    #[serde(default, skip_serializing_if = "TargetProjectConfig::is_default")]
    pub target_project: TargetProjectConfig,
}

/// On-disk format for `.runa/state.toml` — initialization metadata managed by runa.
#[derive(Serialize, Deserialize)]
pub struct State {
    pub initialized_at: String,
    pub runa_version: String,
}

/// A fully loaded runa project: manifest, dependency graph, and artifact store.
pub struct LoadedProject {
    pub config: Config,
    pub manifest: Manifest,
    pub graph: DependencyGraph,
    pub store: ArtifactStore,
    pub workspace_dir: PathBuf,
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
    /// Project config exists but cannot be parsed.
    ProjectConfigParseFailed(String),
    /// `.runa/state.toml` is missing — project not initialized.
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
            ProjectError::ConfigNotFound => {
                write!(f, "no config found (run 'runa init' first)")
            }
            ProjectError::ConfigPathNotFound(path) => {
                write!(f, "config not found: {}", path.display())
            }
            ProjectError::ConfigParseFailed(detail) => {
                write!(f, "failed to parse config: {detail}")
            }
            ProjectError::ProjectConfigParseFailed(detail) => {
                write!(f, "failed to parse project config: {detail}")
            }
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
pub fn resolve_config(
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

/// Resolve and parse the config file into a [`Config`].
///
/// Uses [`resolve_config`] to find the config path, then reads and
/// deserializes the TOML content.
pub fn read_config(
    working_dir: &Path,
    config_override: Option<&Path>,
) -> Result<Config, ProjectError> {
    reject_retired_forge_env()?;
    let config_path = resolve_config(working_dir, config_override)?;
    let config_content = std::fs::read_to_string(&config_path).map_err(ProjectError::Io)?;
    let config_value: toml::Value = toml::from_str(&config_content)
        .map_err(|e| ProjectError::ConfigParseFailed(e.to_string()))?;
    if config_value.get("artifacts_dir").is_some() {
        return Err(ProjectError::ConfigParseFailed(
            "'artifacts_dir' has been removed; artifacts always live under '.runa/workspace/' and are no longer configurable"
                .to_string(),
        ));
    }
    reject_legacy_surface(&config_value).map_err(ProjectError::ConfigParseFailed)?;
    let machine_config: MachineConfig = config_value
        .try_into()
        .map_err(|e| ProjectError::ConfigParseFailed(e.to_string()))?;
    let project_config = read_project_config(working_dir)?;
    Ok(merge_config(machine_config, project_config))
}

fn reject_retired_forge_env() -> Result<(), ProjectError> {
    for name in [
        "RUNA_FORGE_TYPE",
        "RUNA_FORGE_OWNER",
        "RUNA_FORGE_NAME",
        "RUNA_FORGE_TRACKER_ID",
    ] {
        if std::env::var(name)
            .ok()
            .is_some_and(|value| !value.is_empty())
        {
            return Err(ProjectError::ConfigParseFailed(format!(
                "`{name}` has been retired; configure `.runa/project.toml` and use `RUNA_TARGET_PROJECT` in launched environments"
            )));
        }
    }
    Ok(())
}

pub fn read_project_config(working_dir: &Path) -> Result<ProjectConfig, ProjectError> {
    let path = working_dir.join(RUNA_DIR).join(PROJECT_FILENAME);
    if !path.exists() {
        return Ok(ProjectConfig::default());
    }
    let content = std::fs::read_to_string(&path).map_err(ProjectError::Io)?;
    let value: toml::Value = toml::from_str(&content)
        .map_err(|e| ProjectError::ProjectConfigParseFailed(e.to_string()))?;
    reject_legacy_surface(&value).map_err(ProjectError::ProjectConfigParseFailed)?;
    value
        .try_into()
        .map_err(|e| ProjectError::ProjectConfigParseFailed(e.to_string()))
}

fn merge_config(machine: MachineConfig, project: ProjectConfig) -> Config {
    let mut transcript = project.transcript;
    if machine.transcript.dir.is_some() {
        transcript.dir = machine.transcript.dir;
    }
    Config {
        methodology_path: machine.methodology_path,
        logging: machine.logging,
        launch: project.launch,
        transcript,
        target_project: project.target_project,
    }
}

fn reject_legacy_surface(value: &toml::Value) -> Result<(), String> {
    let Some(table) = value.as_table() else {
        return Ok(());
    };
    if table.contains_key("agent") {
        return Err(
            "`[agent]` has been retired; use portable `[launch]` in `.runa/project.toml`"
                .to_string(),
        );
    }
    if table.contains_key("forge") {
        return Err(
            "`[forge]` has been retired; use portable `[target_project]` in `.runa/project.toml`"
                .to_string(),
        );
    }
    Ok(())
}

pub fn target_project_env(
    config: &TargetProjectConfig,
) -> Result<Vec<(String, String)>, ProjectError> {
    let payload = TargetProjectPayload::from(config);
    let encoded = serde_json::to_string(&payload)
        .map_err(|error| ProjectError::ConfigParseFailed(error.to_string()))?;
    Ok(vec![(RUNA_TARGET_PROJECT.to_string(), encoded)])
}

/// Load a runa project from `working_dir`.
///
/// Resolves config via the resolution chain, reads state, parses the methodology
/// manifest, builds the dependency graph, and opens the artifact store.
pub fn load(
    working_dir: &Path,
    config_override: Option<&Path>,
) -> Result<LoadedProject, ProjectError> {
    let config = read_config(working_dir, config_override)?;

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

    let manifest = crate::manifest::parse(Path::new(&config.methodology_path))
        .map_err(ProjectError::ManifestInvalid)?;

    let graph = DependencyGraph::build(&manifest.protocols).map_err(ProjectError::GraphInvalid)?;

    let workspace_dir = runa_dir.join(DEFAULT_WORKSPACE_DIR);
    let store_dir = runa_dir.join(STORE_DIRNAME);
    let mut store = ArtifactStore::new(manifest.artifact_types.clone(), store_dir)
        .map_err(ProjectError::StoreError)?;
    store
        .sync_execution_contract_hash(Some(crate::store::execution_contract_hash(&manifest)))
        .map_err(ProjectError::StoreError)?;

    Ok(LoadedProject {
        config,
        manifest,
        graph,
        store,
        workspace_dir,
    })
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

[[protocols]]
name = "ground"
produces = ["constraints"]
trigger = { type = "on_change", name = "constraints" }
"#
    }

    /// Write a methodology layout alongside the manifest.
    fn write_methodology_layout(manifest_dir: &Path) {
        crate::test_helpers::write_methodology(
            manifest_dir,
            valid_manifest_toml(),
            &[("constraints", r#"{"type": "object"}"#)],
            &["ground"],
        );
    }

    fn write_project_files(working: &Path, manifest_path: &Path) {
        let runa_dir = working.join(".runa");
        fs::create_dir_all(&runa_dir).unwrap();

        let canonical = fs::canonicalize(manifest_path).unwrap();
        let config = MachineConfig {
            methodology_path: canonical.display().to_string(),
            logging: LoggingConfig::default(),
            transcript: TranscriptConfig::default(),
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
        write_methodology_layout(dir.path());
        let manifest_path = dir.path().join("manifest.toml");

        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();
        write_project_files(&working, &manifest_path);

        let loaded = load(&working, None).unwrap();
        assert_eq!(loaded.manifest.name, "test-methodology");
        assert_eq!(loaded.workspace_dir, working.join(".runa/workspace"));
    }

    #[test]
    fn load_with_explicit_config_override() {
        let dir = tempfile::tempdir().unwrap();
        write_methodology_layout(dir.path());
        let manifest_path = dir.path().join("manifest.toml");

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
        let config = MachineConfig {
            methodology_path: canonical.display().to_string(),
            logging: LoggingConfig::default(),
            transcript: TranscriptConfig::default(),
        };
        fs::write(&external_config_path, toml::to_string(&config).unwrap()).unwrap();

        let loaded = load(&working, Some(&external_config_path)).unwrap();
        assert_eq!(loaded.manifest.name, "test-methodology");
    }

    #[test]
    fn load_rejects_removed_artifacts_dir_field() {
        let dir = tempfile::tempdir().unwrap();
        write_methodology_layout(dir.path());
        let manifest_path = dir.path().join("manifest.toml");

        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();

        let runa_dir = working.join(".runa");
        fs::create_dir_all(&runa_dir).unwrap();

        let canonical = fs::canonicalize(&manifest_path).unwrap();
        fs::write(
            runa_dir.join("config.toml"),
            format!(
                r#"
methodology_path = "{}"
artifacts_dir = "custom-artifacts"
"#,
                canonical.display()
            ),
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

        let err = load(&working, None).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("artifacts_dir"),
            "error should name removed field: {message}"
        );
        assert!(
            message.contains(".runa/workspace"),
            "error should name invariant workspace path: {message}"
        );
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
        let config = MachineConfig {
            methodology_path: canonical.display().to_string(),
            logging: LoggingConfig::default(),
            transcript: TranscriptConfig::default(),
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

    #[test]
    fn read_config_rejects_removed_artifacts_dir_field() {
        let dir = tempfile::tempdir().unwrap();
        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();
        let runa_dir = working.join(".runa");
        fs::create_dir_all(&runa_dir).unwrap();
        fs::write(
            runa_dir.join("config.toml"),
            r#"
methodology_path = "/tmp/methodology.toml"
artifacts_dir = "custom-artifacts"
"#,
        )
        .unwrap();

        let err = read_config(&working, None).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("artifacts_dir"),
            "error should name removed field: {message}"
        );
        assert!(
            message.contains(".runa/workspace"),
            "error should name invariant workspace path: {message}"
        );
    }

    #[test]
    fn config_without_logging_uses_default_logging_settings() {
        let config: Config = toml::from_str(
            r#"
methodology_path = "/tmp/methodology.toml"
"#,
        )
        .unwrap();

        assert_eq!(config.logging.format, LogFormat::Text);
        assert_eq!(config.logging.filter, None);
        assert_eq!(config.launch.command, None);
    }

    #[test]
    fn config_with_logging_table_parses_format_and_filter() {
        let config: Config = toml::from_str(
            r#"
methodology_path = "/tmp/methodology.toml"

[logging]
format = "json"
filter = "info"
"#,
        )
        .unwrap();

        assert_eq!(config.logging.format, LogFormat::Json);
        assert_eq!(config.logging.filter.as_deref(), Some("info"));
        assert_eq!(config.launch.command, None);
    }

    #[test]
    fn project_config_with_launch_table_parses_command() {
        let config: ProjectConfig = toml::from_str(
            r#"
[launch]
command = ["agent-runtime", "exec"]
"#,
        )
        .unwrap();

        assert_eq!(
            config.launch.command,
            Some(vec!["agent-runtime".to_string(), "exec".to_string()])
        );
    }

    #[test]
    fn split_config_combines_machine_and_portable_runtime_settings() {
        let dir = tempfile::tempdir().unwrap();
        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();
        let runa_dir = working.join(".runa");
        fs::create_dir_all(&runa_dir).unwrap();
        fs::write(
            runa_dir.join("config.toml"),
            r#"
methodology_path = "/tmp/methodology.toml"

[transcript]
dir = "var/transcripts"
            "#,
        )
        .unwrap();
        fs::write(
            runa_dir.join("project.toml"),
            r#"
[transcript]
redact_env = ["SECRET_TOKEN", "API_KEY"]

[target_project]
forge_type = "sourcehut"

[[target_project.repositories]]
selector = "groundwork"
host = "weforge.build"
owner = "operator"
name = "weforge"

[[target_project.trackers]]
selector = "todo"
host = "weforge.build"
owner = "operator"
name = "weforge"
tracker_id = "4"
"#,
        )
        .unwrap();

        let config = read_config(&working, None).unwrap();

        assert_eq!(config.transcript.dir.as_deref(), Some("var/transcripts"));
        assert_eq!(config.transcript.redact_env, ["SECRET_TOKEN", "API_KEY"]);
        assert_eq!(config.target_project.forge_type, ForgeType::Sourcehut);
        assert_eq!(config.target_project.repositories[0].selector, "groundwork");
        assert_eq!(
            config.target_project.trackers[0].tracker_id.as_deref(),
            Some("4")
        );
    }

    #[test]
    fn config_serializes_and_deserializes_new_durable_runtime_settings() {
        let config = Config {
            methodology_path: "/tmp/methodology.toml".to_string(),
            logging: LoggingConfig::default(),
            launch: LaunchConfig {
                command: Some(vec!["agent-runtime".to_string(), "exec".to_string()]),
            },
            transcript: TranscriptConfig {
                dir: Some("transcripts".to_string()),
                redact_env: vec!["SECRET_TOKEN".to_string()],
            },
            target_project: TargetProjectConfig {
                forge_type: ForgeType::Github,
                repositories: vec![RepositoryConfig {
                    selector: "runa".to_string(),
                    host: "github.com".to_string(),
                    owner: "tesserine".to_string(),
                    name: "runa".to_string(),
                }],
                trackers: vec![TrackerConfig {
                    selector: "issues".to_string(),
                    repository: Some("runa".to_string()),
                    host: None,
                    owner: None,
                    name: None,
                    tracker_id: None,
                }],
            },
        };

        let serialized = toml::to_string(&config).unwrap();
        let round_tripped: Config = toml::from_str(&serialized).unwrap();

        assert_eq!(round_tripped, config);
    }

    #[test]
    fn read_config_rejects_legacy_agent_and_forge_tables() {
        for legacy in [
            "[agent]\ncommand = [\"agent\"]\n",
            "[forge]\ntype = \"github\"\n",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let working = dir.path().join("project");
            fs::create_dir(&working).unwrap();
            let runa_dir = working.join(".runa");
            fs::create_dir_all(&runa_dir).unwrap();
            fs::write(
                runa_dir.join("config.toml"),
                format!("methodology_path = \"/tmp/methodology.toml\"\n{legacy}"),
            )
            .unwrap();

            let err = read_config(&working, None).unwrap_err();
            assert!(err.to_string().contains("retired"), "{err}");
        }
    }
}
