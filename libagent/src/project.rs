//! Shared project loading logic used by both `runa-cli` and `runa-mcp`.
//!
//! Resolves the config file via the resolution chain, parses the methodology
//! manifest, builds the dependency graph, and initializes the artifact store.
//! See [`load`] for the main entry point.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::forge_address::{ForgeAddressError, ForgeProject, RawForges, reject_legacy_environment};
use crate::{ArtifactStore, DependencyGraph, GraphError, Manifest, ManifestError, StoreError};

pub const RUNA_DIR: &str = ".runa";
pub const CONFIG_FILENAME: &str = "config.toml";
pub const PROJECT_FILENAME: &str = "project.toml";
pub const STATE_FILENAME: &str = "state.toml";
pub const DEFAULT_WORKSPACE_DIR: &str = "workspace";
pub const STORE_DIRNAME: &str = "store";

/// Output format for tracing events written to stderr.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LogFormat {
    #[default]
    Text,
    Json,
}

/// The `[logging]` section of portable `.runa/project.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
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

/// Effective launch command, sourced from `[launch]` in the portable project file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
}

impl AgentConfig {
    fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

/// Effective transcript config. `dir` is machine-local; `redact_env` is portable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct TranscriptConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redact_env: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct DeploymentConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
}

impl DeploymentConfig {
    fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

/// Effective project configuration built from local and portable files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub methodology_path: String,
    pub logging: LoggingConfig,
    pub agent: AgentConfig,
    pub transcript: TranscriptConfig,
    pub deployment: DeploymentConfig,
    pub forge: ForgeProject,
    pub project_config_path: PathBuf,
}

/// On-disk format for `.runa/config.toml` — machine-local paths and directories.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalConfig {
    pub methodology_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_config: Option<String>,
    #[serde(default, skip_serializing_if = "LocalTranscriptConfig::is_default")]
    pub transcript: LocalTranscriptConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct LocalTranscriptConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
}

impl LocalTranscriptConfig {
    fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

/// On-disk format for `.runa/project.toml` — portable project surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableConfig {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "LoggingConfig::is_default")]
    pub logging: LoggingConfig,
    #[serde(default, skip_serializing_if = "AgentConfig::is_default")]
    pub launch: AgentConfig,
    #[serde(default, skip_serializing_if = "PortableTranscriptConfig::is_default")]
    pub transcript: PortableTranscriptConfig,
    #[serde(default, skip_serializing_if = "DeploymentConfig::is_default")]
    pub deployment: DeploymentConfig,
    #[serde(default)]
    pub forges: RawForges,
}

impl Default for PortableConfig {
    fn default() -> Self {
        Self {
            schema_version: default_schema_version(),
            logging: LoggingConfig::default(),
            launch: AgentConfig::default(),
            transcript: PortableTranscriptConfig::default(),
            deployment: DeploymentConfig::default(),
            forges: RawForges::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PortableTranscriptConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redact_env: Vec<String>,
}

impl PortableTranscriptConfig {
    fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

fn default_schema_version() -> u32 {
    1
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
    /// Portable forge-address contract is invalid.
    ForgeAddressInvalid(ForgeAddressError),
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
            ProjectError::ForgeAddressInvalid(error) => write!(f, "{error}"),
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
            ProjectError::ForgeAddressInvalid(e) => Some(e),
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
    reject_legacy_environment().map_err(ProjectError::ForgeAddressInvalid)?;
    let config_path = resolve_config(working_dir, config_override)?;
    let config_content = std::fs::read_to_string(&config_path).map_err(ProjectError::Io)?;
    let config_value: toml::Value = toml::from_str(&config_content)
        .map_err(|e| ProjectError::ConfigParseFailed(e.to_string()))?;
    reject_removed_and_legacy_local_settings(&config_value)?;
    let local: LocalConfig = config_value
        .try_into()
        .map_err(|e| ProjectError::ConfigParseFailed(e.to_string()))?;

    let project_config_path = resolve_project_config_path(working_dir, &config_path, &local);
    let portable = read_portable_config(&project_config_path)?;
    let forge =
        ForgeProject::resolve(portable.forges).map_err(ProjectError::ForgeAddressInvalid)?;
    validate_deployment_repository(&portable.deployment, &forge)
        .map_err(ProjectError::ForgeAddressInvalid)?;

    Ok(Config {
        methodology_path: local.methodology_path,
        logging: portable.logging,
        agent: portable.launch,
        transcript: TranscriptConfig {
            dir: local.transcript.dir,
            redact_env: portable.transcript.redact_env,
        },
        deployment: portable.deployment,
        forge,
        project_config_path,
    })
}

fn validate_deployment_repository(
    deployment: &DeploymentConfig,
    forge: &ForgeProject,
) -> Result<(), ForgeAddressError> {
    if let Some(selector) = deployment.repository.as_deref() {
        return forge.deployment_identity(Some(selector)).map(|_| ());
    }
    if forge.repositories.is_empty() {
        return Ok(());
    }
    if forge.repositories.len() == 1 {
        return Ok(());
    }
    Err(ForgeAddressError::AmbiguousDeploymentRepository)
}

fn read_portable_config(path: &Path) -> Result<PortableConfig, ProjectError> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PortableConfig::default());
        }
        Err(error) => return Err(ProjectError::Io(error)),
    };
    let value: toml::Value =
        toml::from_str(&content).map_err(|e| ProjectError::ConfigParseFailed(e.to_string()))?;
    reject_machine_local_portable_settings(&value)?;
    let portable: PortableConfig = value
        .try_into()
        .map_err(|e| ProjectError::ConfigParseFailed(e.to_string()))?;
    if portable.schema_version != 1 {
        return Err(ProjectError::ConfigParseFailed(format!(
            "unsupported project config schema_version {}; expected 1",
            portable.schema_version
        )));
    }
    Ok(portable)
}

fn resolve_project_config_path(
    working_dir: &Path,
    config_path: &Path,
    local: &LocalConfig,
) -> PathBuf {
    let Some(path) = local.project_config.as_deref() else {
        return working_dir.join(RUNA_DIR).join(PROJECT_FILENAME);
    };
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        config_path.parent().unwrap_or(working_dir).join(path)
    }
}

fn reject_removed_and_legacy_local_settings(value: &toml::Value) -> Result<(), ProjectError> {
    if value.get("artifacts_dir").is_some() {
        return Err(ProjectError::ConfigParseFailed(
            "'artifacts_dir' has been removed; artifacts always live under '.runa/workspace/' and are no longer configurable"
                .to_string(),
        ));
    }
    for legacy in ["agent", "forge"] {
        if value.get(legacy).is_some() {
            return Err(ProjectError::ConfigParseFailed(format!(
                "'[{legacy}]' has been removed; configure launch and forge addresses in '.runa/project.toml' using '[launch]' and '[forges.*]'"
            )));
        }
    }
    reject_unknown_local_settings(value)?;
    if let Some(transcript) = value.get("transcript").and_then(toml::Value::as_table) {
        for key in transcript.keys() {
            if key != "dir" {
                return Err(ProjectError::ConfigParseFailed(format!(
                    "'[transcript].{key}' belongs in portable '.runa/project.toml', not machine-local '.runa/config.toml'"
                )));
            }
        }
    }
    Ok(())
}

fn reject_unknown_local_settings(value: &toml::Value) -> Result<(), ProjectError> {
    let Some(table) = value.as_table() else {
        return Ok(());
    };
    for (key, nested) in table {
        if matches!(
            key.as_str(),
            "methodology_path" | "project_config" | "transcript"
        ) {
            continue;
        }
        return Err(ProjectError::ConfigParseFailed(format!(
            "'{}' is not a machine-local setting; portable project settings belong in '.runa/project.toml', not machine-local '.runa/config.toml'",
            config_key_display(key, nested)
        )));
    }
    Ok(())
}

fn config_key_display(key: &str, value: &toml::Value) -> String {
    if value.as_table().is_some() {
        format!("[{key}]")
    } else {
        key.to_string()
    }
}

fn reject_machine_local_portable_settings(value: &toml::Value) -> Result<(), ProjectError> {
    for local in ["methodology_path", "project_config"] {
        if value.get(local).is_some() {
            return Err(ProjectError::ConfigParseFailed(format!(
                "'{local}' belongs in machine-local '.runa/config.toml', not portable '.runa/project.toml'"
            )));
        }
    }
    if value
        .get("transcript")
        .and_then(|section| section.get("dir"))
        .is_some()
    {
        return Err(ProjectError::ConfigParseFailed(
            "'[transcript].dir' belongs in machine-local '.runa/config.toml', not portable '.runa/project.toml'"
                .to_string(),
        ));
    }
    if value.get("agent").is_some() || value.get("forge").is_some() {
        return Err(ProjectError::ConfigParseFailed(
            "legacy '[agent]' and '[forge]' sections are not accepted; use '[launch]' and '[forges.*]' in '.runa/project.toml'"
                .to_string(),
        ));
    }
    Ok(())
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
        fs::write(
            runa_dir.join("config.toml"),
            format!("methodology_path = {:?}\n", canonical.display().to_string()),
        )
        .unwrap();
        fs::write(runa_dir.join("project.toml"), "schema_version = 1\n").unwrap();

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
        fs::write(
            &external_config_path,
            format!("methodology_path = {:?}\n", canonical.display().to_string()),
        )
        .unwrap();

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
        fs::write(
            runa_dir.join("config.toml"),
            format!("methodology_path = {:?}\n", canonical.display().to_string()),
        )
        .unwrap();
        fs::write(runa_dir.join("project.toml"), "schema_version = 1\n").unwrap();

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
    fn read_config_rejects_portable_launch_section_in_local_config() {
        let dir = tempfile::tempdir().unwrap();
        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();
        let runa_dir = working.join(".runa");
        fs::create_dir_all(&runa_dir).unwrap();
        fs::write(
            runa_dir.join("config.toml"),
            r#"
methodology_path = "/tmp/methodology.toml"

[launch]
command = ["agent-runtime", "exec"]
"#,
        )
        .unwrap();

        let err = read_config(&working, None).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("[launch]"),
            "error should name misplaced section: {message}"
        );
        assert!(
            message.contains(".runa/project.toml"),
            "error should name portable project file: {message}"
        );
    }

    #[test]
    fn read_config_rejects_unknown_top_level_key_in_local_config() {
        let dir = tempfile::tempdir().unwrap();
        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();
        let runa_dir = working.join(".runa");
        fs::create_dir_all(&runa_dir).unwrap();
        fs::write(
            runa_dir.join("config.toml"),
            r#"
methodology_path = "/tmp/methodology.toml"
future_portable = true
"#,
        )
        .unwrap();

        let err = read_config(&working, None).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("future_portable"),
            "error should name unknown key: {message}"
        );
        assert!(
            message.contains(".runa/project.toml"),
            "error should direct portable settings to project file: {message}"
        );
    }

    #[test]
    fn read_config_rejects_unknown_nested_key_in_local_transcript() {
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
redact_env = ["SECRET_TOKEN"]
"#,
        )
        .unwrap();

        let err = read_config(&working, None).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("[transcript].redact_env"),
            "error should name misplaced nested key: {message}"
        );
        assert!(
            message.contains(".runa/project.toml"),
            "error should name portable project file: {message}"
        );
    }

    #[test]
    fn config_without_project_file_uses_default_portable_settings() {
        let dir = tempfile::tempdir().unwrap();
        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();
        let runa_dir = working.join(".runa");
        fs::create_dir_all(&runa_dir).unwrap();
        fs::write(
            runa_dir.join("config.toml"),
            r#"methodology_path = "/tmp/methodology.toml""#,
        )
        .unwrap();

        let config = read_config(&working, None).unwrap();

        assert_eq!(config.logging.format, LogFormat::Text);
        assert_eq!(config.logging.filter, None);
        assert_eq!(config.agent.command, None);
        assert!(config.forge.instances.is_empty());
    }

    #[test]
    fn split_config_parses_local_and_portable_settings() {
        let dir = tempfile::tempdir().unwrap();
        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();
        let runa_dir = working.join(".runa");
        fs::create_dir_all(&runa_dir).unwrap();
        fs::write(
            runa_dir.join("config.toml"),
            r#"
methodology_path = "/tmp/methodology.toml"
project_config = "project.toml"

[transcript]
dir = "var/transcripts"
"#,
        )
        .unwrap();
        fs::write(
            runa_dir.join("project.toml"),
            r#"
schema_version = 1

[logging]
format = "json"
filter = "info"

[launch]
command = ["agent-runtime", "exec"]

[transcript]
redact_env = ["SECRET_TOKEN", "API_KEY"]

[[forges.instances]]
id = "weforge"
type = "sourcehut"
git_host = "git.weforge.build"
tracker_host = "todo.weforge.build"

[[forges.trackers]]
id = "weforge"
instance = "weforge"
owner = "operator"
name = "weforge"
tracker_id = "4"
"#,
        )
        .unwrap();

        let config = read_config(&working, None).unwrap();

        assert_eq!(config.logging.format, LogFormat::Json);
        assert_eq!(config.logging.filter.as_deref(), Some("info"));
        assert_eq!(
            config.agent.command,
            Some(vec!["agent-runtime".to_string(), "exec".to_string()])
        );
        assert_eq!(config.transcript.dir.as_deref(), Some("var/transcripts"));
        assert_eq!(config.transcript.redact_env, ["SECRET_TOKEN", "API_KEY"]);
        assert_eq!(config.forge.instances[0].forge_type.as_str(), "sourcehut");
        assert_eq!(config.forge.trackers[0].tracker_id.as_deref(), Some("4"));
    }

    #[test]
    fn read_config_rejects_machine_local_methodology_path_in_portable_config() {
        let dir = tempfile::tempdir().unwrap();
        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();
        let runa_dir = working.join(".runa");
        fs::create_dir_all(&runa_dir).unwrap();
        fs::write(
            runa_dir.join("config.toml"),
            r#"methodology_path = "/tmp/methodology.toml""#,
        )
        .unwrap();
        fs::write(
            runa_dir.join("project.toml"),
            r#"
schema_version = 1
methodology_path = "/tmp/other-methodology.toml"
"#,
        )
        .unwrap();

        let err = read_config(&working, None).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("methodology_path"),
            "error should name misplaced local key: {message}"
        );
        assert!(
            message.contains(".runa/config.toml"),
            "error should name machine-local config file: {message}"
        );
    }

    #[test]
    fn read_config_rejects_unknown_nested_key_in_portable_logging() {
        let dir = tempfile::tempdir().unwrap();
        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();
        let runa_dir = working.join(".runa");
        fs::create_dir_all(&runa_dir).unwrap();
        fs::write(
            runa_dir.join("config.toml"),
            r#"methodology_path = "/tmp/methodology.toml""#,
        )
        .unwrap();
        fs::write(
            runa_dir.join("project.toml"),
            r#"
schema_version = 1

[logging]
format = "json"
extra = "ignored"
"#,
        )
        .unwrap();

        let err = read_config(&working, None).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("extra"),
            "error should name unknown nested field: {message}"
        );
    }

    #[test]
    fn config_resolution_rejects_unsupported_forge_type() {
        let dir = tempfile::tempdir().unwrap();
        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();
        let runa_dir = working.join(".runa");
        fs::create_dir_all(&runa_dir).unwrap();
        fs::write(
            runa_dir.join("config.toml"),
            r#"methodology_path = "/tmp/methodology.toml""#,
        )
        .unwrap();
        fs::write(
            runa_dir.join("project.toml"),
            r#"
schema_version = 1

[[forges.instances]]
id = "gitlab"
type = "gitlab"
host = "gitlab.example"
"#,
        )
        .unwrap();

        let error = read_config(&working, None).unwrap_err();

        assert!(matches!(
            error,
            ProjectError::ForgeAddressInvalid(ForgeAddressError::UnsupportedForgeType { forge_type })
                if forge_type == "gitlab"
        ));
    }

    #[test]
    fn single_repository_resolves_deployment_without_selector() {
        let dir = tempfile::tempdir().unwrap();
        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();
        let runa_dir = working.join(".runa");
        fs::create_dir_all(&runa_dir).unwrap();
        fs::write(
            runa_dir.join("config.toml"),
            r#"methodology_path = "/tmp/methodology.toml""#,
        )
        .unwrap();
        fs::write(
            runa_dir.join("project.toml"),
            r#"
schema_version = 1

[[forges.instances]]
id = "github-com"
type = "github"
host = "github.com"

[[forges.repositories]]
id = "runa"
instance = "github-com"
owner = "tesserine"
name = "runa"
"#,
        )
        .unwrap();

        let config = read_config(&working, None).unwrap();

        assert_eq!(
            config.forge.deployment_identity(None).unwrap(),
            "github@github.com/repo/tesserine/runa"
        );
    }

    #[test]
    fn read_config_rejects_unknown_deployment_repository_selector() {
        let dir = tempfile::tempdir().unwrap();
        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();
        let runa_dir = working.join(".runa");
        fs::create_dir_all(&runa_dir).unwrap();
        fs::write(
            runa_dir.join("config.toml"),
            r#"methodology_path = "/tmp/methodology.toml""#,
        )
        .unwrap();
        fs::write(
            runa_dir.join("project.toml"),
            r#"
schema_version = 1

[deployment]
repository = "missing"
"#,
        )
        .unwrap();

        let err = read_config(&working, None).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("missing"),
            "error should name unresolved deployment selector: {message}"
        );
    }
}
