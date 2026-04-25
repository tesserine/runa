use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::project::{
    CONFIG_FILENAME, Config, DEFAULT_WORKSPACE_DIR, RUNA_DIR, STATE_FILENAME, STORE_DIRNAME, State,
};

#[derive(Debug)]
pub struct InitSummary {
    pub methodology_name: String,
    pub artifact_type_count: usize,
    pub protocol_count: usize,
}

#[derive(Debug)]
pub enum InitError {
    MethodologyNotFound { path: PathBuf },
    ManifestInvalid(libagent::ManifestError),
    ExistingRunaPathUnusable(ExistingRunaPathDiagnostic),
    Io(std::io::Error),
}

#[derive(Debug)]
pub struct ExistingRunaPathDiagnostic {
    path: PathBuf,
    owner_uid: u32,
    current_uid: u32,
    reason: ExistingRunaPathReason,
}

#[derive(Debug)]
enum ExistingRunaPathReason {
    OwnerMismatch,
    NotWritable,
}

impl fmt::Display for InitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InitError::MethodologyNotFound { path } => {
                write!(f, "methodology not found: {}", path.display())
            }
            InitError::ManifestInvalid(e) => write!(f, "{e}"),
            InitError::ExistingRunaPathUnusable(diagnostic) => write!(f, "{diagnostic}"),
            InitError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl fmt::Display for ExistingRunaPathDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self.reason {
            ExistingRunaPathReason::OwnerMismatch => "is owned by a different user",
            ExistingRunaPathReason::NotWritable => "is not writable by the current user",
        };
        write!(
            f,
            "pre-existing runa state is not usable: {} {reason} \
             (owned by uid {}, current uid {}). \
             This usually means the directory is managed by another tool such as agentd, \
             was left behind by a sudo-created init, or this command is running in the wrong directory. \
             If another tool manages this directory, do not run `runa init` here. \
             If this is leftover state, remove it and retry, or run `runa init` in the intended project directory.",
            self.path.display(),
            self.owner_uid,
            self.current_uid
        )
    }
}

impl std::error::Error for InitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            InitError::ManifestInvalid(e) => Some(e),
            InitError::Io(e) => Some(e),
            InitError::MethodologyNotFound { .. } | InitError::ExistingRunaPathUnusable(_) => None,
        }
    }
}

/// Run `runa init`.
///
/// `config_path` is where to write the config file. When `--config` is provided,
/// it points there; otherwise it defaults to `.runa/config.toml` in working_dir.
pub fn run(
    working_dir: &Path,
    methodology: &Path,
    config_path: Option<&Path>,
) -> Result<InitSummary, InitError> {
    if !methodology.exists() {
        return Err(InitError::MethodologyNotFound {
            path: methodology.to_path_buf(),
        });
    }

    let manifest = libagent::manifest::parse(methodology).map_err(InitError::ManifestInvalid)?;

    let canonical_path = fs::canonicalize(methodology).map_err(InitError::Io)?;

    let runa_dir = working_dir.join(RUNA_DIR);
    let config_dest = match config_path {
        Some(p) => p.to_path_buf(),
        None => runa_dir.join(CONFIG_FILENAME),
    };
    let config_dest_for_preflight = if config_dest.is_absolute() {
        config_dest.clone()
    } else {
        working_dir.join(&config_dest)
    };
    preflight_existing_runa_paths(&runa_dir, &config_dest_for_preflight)?;

    fs::create_dir_all(&runa_dir).map_err(InitError::Io)?;
    fs::create_dir_all(runa_dir.join(STORE_DIRNAME)).map_err(InitError::Io)?;

    let workspace_dir = runa_dir.join(DEFAULT_WORKSPACE_DIR);
    fs::create_dir_all(&workspace_dir).map_err(InitError::Io)?;

    // Write config.
    let config = Config {
        methodology_path: canonical_path.display().to_string(),
        logging: crate::project::LoggingConfig::default(),
        agent: crate::project::AgentConfig::default(),
    };
    let config_toml = toml::to_string(&config).expect("Config serialization should not fail");

    if let Some(parent) = config_dest.parent() {
        fs::create_dir_all(parent).map_err(InitError::Io)?;
    }
    fs::write(&config_dest, config_toml).map_err(InitError::Io)?;

    // Write state.
    let state = State {
        initialized_at: now_iso8601(),
        runa_version: env!("CARGO_PKG_VERSION").to_string(),
    };
    let state_toml = toml::to_string(&state).expect("State serialization should not fail");
    fs::write(runa_dir.join(STATE_FILENAME), state_toml).map_err(InitError::Io)?;

    Ok(InitSummary {
        methodology_name: manifest.name,
        artifact_type_count: manifest.artifact_types.len(),
        protocol_count: manifest.protocols.len(),
    })
}

fn preflight_existing_runa_paths(runa_dir: &Path, config_dest: &Path) -> Result<(), InitError> {
    let mut paths = vec![
        runa_dir.to_path_buf(),
        runa_dir.join(STORE_DIRNAME),
        runa_dir.join(DEFAULT_WORKSPACE_DIR),
        runa_dir.join(STATE_FILENAME),
    ];
    if config_dest.starts_with(runa_dir)
        && let Some(parent) = config_dest.parent()
    {
        paths.push(parent.to_path_buf());
    }

    for path in paths {
        preflight_existing_runa_path(&path)?;
    }

    Ok(())
}

#[cfg(unix)]
fn preflight_existing_runa_path(path: &Path) -> Result<(), InitError> {
    use std::os::unix::fs::MetadataExt;

    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(InitError::Io(error)),
    };

    if let Some(diagnostic) = diagnose_existing_runa_path(
        path,
        metadata.uid(),
        current_uid(),
        owner_can_write(&metadata),
    ) {
        return Err(InitError::ExistingRunaPathUnusable(diagnostic));
    }

    Ok(())
}

fn diagnose_existing_runa_path(
    path: &Path,
    owner_uid: u32,
    current_uid: u32,
    current_user_can_write: bool,
) -> Option<ExistingRunaPathDiagnostic> {
    let reason = if owner_uid != current_uid {
        Some(ExistingRunaPathReason::OwnerMismatch)
    } else if !current_user_can_write {
        Some(ExistingRunaPathReason::NotWritable)
    } else {
        None
    };

    reason.map(|reason| ExistingRunaPathDiagnostic {
        path: path.to_path_buf(),
        owner_uid,
        current_uid,
        reason,
    })
}

#[cfg(not(unix))]
fn preflight_existing_runa_path(_path: &Path) -> Result<(), InitError> {
    Ok(())
}

#[cfg(unix)]
fn current_uid() -> u32 {
    // SAFETY: `geteuid` has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

#[cfg(unix)]
fn owner_can_write(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    let mode = metadata.mode();
    if metadata.file_type().is_dir() {
        mode & 0o300 == 0o300
    } else {
        mode & 0o200 == 0o200
    }
}

fn now_iso8601() -> String {
    // UTC timestamp without external dependencies.
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before UNIX epoch");
    let secs = duration.as_secs();

    // Convert to calendar components (UTC).
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Days since 1970-01-01 to (year, month, day) — civil calendar algorithm.
    let (year, month, day) = days_to_date(days);

    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

/// Convert days since 1970-01-01 to (year, month, day).
/// Uses the algorithm from Howard Hinnant's date library.
fn days_to_date(days: u64) -> (u64, u64, u64) {
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}
#[cfg(test)]
mod tests {
    use super::*;

    fn valid_manifest_toml() -> &'static str {
        r#"
name = "groundwork"

[[artifact_types]]
name = "constraints"

[[artifact_types]]
name = "design-doc"

[[protocols]]
name = "ground"
produces = ["constraints"]
trigger = { type = "on_change", name = "constraints" }

[[protocols]]
name = "design"
requires = ["constraints"]
produces = ["design-doc"]
trigger = { type = "on_artifact", name = "constraints" }

[[protocols]]
name = "review"
requires = ["design-doc"]
trigger = { type = "on_artifact", name = "design-doc" }
"#
    }

    fn write_methodology_layout(dir: &std::path::Path) -> std::path::PathBuf {
        let manifest_path = dir.join("manifest.toml");
        fs::write(&manifest_path, valid_manifest_toml()).unwrap();
        let schemas_dir = dir.join("schemas");
        fs::create_dir_all(&schemas_dir).unwrap();
        for name in &["constraints", "design-doc"] {
            fs::write(
                schemas_dir.join(format!("{name}.schema.json")),
                r#"{"type": "object"}"#,
            )
            .unwrap();
        }
        for protocol in &["ground", "design", "review"] {
            let protocol_dir = dir.join("protocols").join(protocol);
            fs::create_dir_all(&protocol_dir).unwrap();
            fs::write(protocol_dir.join("PROTOCOL.md"), format!("# {protocol}\n")).unwrap();
        }
        manifest_path
    }

    #[test]
    fn valid_manifest_creates_config_and_state_files() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = write_methodology_layout(dir.path());

        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();

        let summary = run(&working, &manifest_path, None).unwrap();

        assert_eq!(summary.methodology_name, "groundwork");
        assert_eq!(summary.artifact_type_count, 2);
        assert_eq!(summary.protocol_count, 3);

        let runa_dir = working.join(".runa");
        assert!(runa_dir.is_dir());
        assert!(runa_dir.join("config.toml").is_file());
        assert!(runa_dir.join("state.toml").is_file());
    }

    #[test]
    fn config_file_records_methodology_path() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = write_methodology_layout(dir.path());

        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();

        run(&working, &manifest_path, None).unwrap();

        let config_content = fs::read_to_string(working.join(".runa").join("config.toml")).unwrap();
        let canonical = fs::canonicalize(&manifest_path).unwrap();
        assert!(
            config_content.contains(&canonical.display().to_string()),
            "config file should contain canonical methodology path"
        );
    }

    #[test]
    fn config_file_omits_artifacts_dir_when_not_provided() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = write_methodology_layout(dir.path());

        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();

        run(&working, &manifest_path, None).unwrap();

        let config_content = fs::read_to_string(working.join(".runa").join("config.toml")).unwrap();
        assert!(
            !config_content.contains("artifacts_dir"),
            "config file should not contain artifacts_dir when not provided"
        );
    }

    #[test]
    fn state_file_records_version_and_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = write_methodology_layout(dir.path());

        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();

        run(&working, &manifest_path, None).unwrap();

        let state_content = fs::read_to_string(working.join(".runa").join("state.toml")).unwrap();
        let state: State = toml::from_str(&state_content).unwrap();
        assert_eq!(state.runa_version, env!("CARGO_PKG_VERSION"));
        assert!(
            state.initialized_at.ends_with('Z'),
            "timestamp should be UTC ISO 8601"
        );
    }

    #[test]
    fn state_file_does_not_contain_methodology_path() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = write_methodology_layout(dir.path());

        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();

        run(&working, &manifest_path, None).unwrap();

        let state_content = fs::read_to_string(working.join(".runa").join("state.toml")).unwrap();
        assert!(
            !state_content.contains("methodology_path"),
            "state file should not contain methodology_path"
        );
    }

    #[test]
    fn custom_config_path_writes_config_there() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = write_methodology_layout(dir.path());

        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();

        let custom_config = dir.path().join("custom").join("config.toml");
        run(&working, &manifest_path, Some(&custom_config)).unwrap();

        assert!(custom_config.is_file(), "config should be at custom path");
        // Default location should not exist.
        assert!(
            !working.join(".runa").join("config.toml").is_file(),
            "default config should not be created when custom path is given"
        );
        // State should still be in the project.
        assert!(working.join(".runa").join("state.toml").is_file());
    }

    #[test]
    fn nonexistent_methodology_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let bogus = dir.path().join("no-such-file.toml");

        let err = run(dir.path(), &bogus, None).unwrap_err();
        assert!(
            matches!(err, InitError::MethodologyNotFound { .. }),
            "expected MethodologyNotFound, got: {err}"
        );
    }

    #[test]
    fn invalid_manifest_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("bad.toml");
        fs::write(&manifest_path, "not valid manifest").unwrap();

        let err = run(dir.path(), &manifest_path, None).unwrap_err();
        assert!(
            matches!(err, InitError::ManifestInvalid(_)),
            "expected ManifestInvalid, got: {err}"
        );
    }

    #[test]
    fn existing_runa_path_diagnostic_reports_owner_mismatch() {
        let diagnostic = diagnose_existing_runa_path(Path::new(".runa"), 0, 1000, true).unwrap();

        let message = diagnostic.to_string();
        assert!(message.contains(".runa"), "message: {message}");
        assert!(message.contains("owned by uid 0"), "message: {message}");
        assert!(message.contains("current uid 1000"), "message: {message}");
        assert!(message.contains("different user"), "message: {message}");
        assert!(message.contains("agentd"), "message: {message}");
        assert!(message.contains("remove"), "message: {message}");
    }

    #[test]
    fn days_to_date_known_timestamps() {
        // 2025-03-13 = 20160 days since 1970-01-01
        assert_eq!(days_to_date(20160), (2025, 3, 13));
        // Unix epoch: 1970-01-01 = day 0
        assert_eq!(days_to_date(0), (1970, 1, 1));
        // 2000-02-29 (leap day) = 11016 days since epoch
        assert_eq!(days_to_date(11016), (2000, 2, 29));
        // 2000-03-01 = 11017 days
        assert_eq!(days_to_date(11017), (2000, 3, 1));
    }

    #[test]
    fn idempotent_run_succeeds_twice() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = write_methodology_layout(dir.path());

        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();

        let summary1 = run(&working, &manifest_path, None).unwrap();
        let summary2 = run(&working, &manifest_path, None).unwrap();

        assert_eq!(summary1.methodology_name, summary2.methodology_name);
        assert_eq!(summary1.artifact_type_count, summary2.artifact_type_count);
        assert_eq!(summary1.protocol_count, summary2.protocol_count);
    }
}
