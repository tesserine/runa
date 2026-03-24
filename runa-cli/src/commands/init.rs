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
    Io(std::io::Error),
}

impl fmt::Display for InitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InitError::MethodologyNotFound { path } => {
                write!(f, "methodology not found: {}", path.display())
            }
            InitError::ManifestInvalid(e) => write!(f, "{e}"),
            InitError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for InitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            InitError::ManifestInvalid(e) => Some(e),
            InitError::Io(e) => Some(e),
            InitError::MethodologyNotFound { .. } => None,
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
    artifacts_dir: Option<&str>,
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
    fs::create_dir_all(&runa_dir).map_err(InitError::Io)?;
    fs::create_dir_all(runa_dir.join(STORE_DIRNAME)).map_err(InitError::Io)?;

    let workspace_dir = artifacts_dir
        .map(|dir| working_dir.join(dir))
        .unwrap_or_else(|| runa_dir.join(DEFAULT_WORKSPACE_DIR));
    fs::create_dir_all(&workspace_dir).map_err(InitError::Io)?;

    // Write config.
    let config = Config {
        methodology_path: canonical_path.display().to_string(),
        artifacts_dir: artifacts_dir.map(String::from),
        logging: crate::project::LoggingConfig::default(),
    };
    let config_toml = toml::to_string(&config).expect("Config serialization should not fail");

    let config_dest = match config_path {
        Some(p) => p.to_path_buf(),
        None => runa_dir.join(CONFIG_FILENAME),
    };
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
schema = { type = "object" }

[[artifact_types]]
name = "design-doc"
schema = { type = "object" }

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

    #[test]
    fn valid_manifest_creates_config_and_state_files() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("manifest.toml");
        fs::write(&manifest_path, valid_manifest_toml()).unwrap();

        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();

        let summary = run(&working, &manifest_path, None, None).unwrap();

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
        let manifest_path = dir.path().join("manifest.toml");
        fs::write(&manifest_path, valid_manifest_toml()).unwrap();

        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();

        run(&working, &manifest_path, None, None).unwrap();

        let config_content = fs::read_to_string(working.join(".runa").join("config.toml")).unwrap();
        let canonical = fs::canonicalize(&manifest_path).unwrap();
        assert!(
            config_content.contains(&canonical.display().to_string()),
            "config file should contain canonical methodology path"
        );
    }

    #[test]
    fn config_file_records_artifacts_dir_when_provided() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("manifest.toml");
        fs::write(&manifest_path, valid_manifest_toml()).unwrap();

        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();

        run(&working, &manifest_path, Some("my-artifacts"), None).unwrap();

        let config_content = fs::read_to_string(working.join(".runa").join("config.toml")).unwrap();
        assert!(
            config_content.contains("my-artifacts"),
            "config file should contain custom artifacts_dir"
        );
    }

    #[test]
    fn config_file_omits_artifacts_dir_when_not_provided() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("manifest.toml");
        fs::write(&manifest_path, valid_manifest_toml()).unwrap();

        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();

        run(&working, &manifest_path, None, None).unwrap();

        let config_content = fs::read_to_string(working.join(".runa").join("config.toml")).unwrap();
        assert!(
            !config_content.contains("artifacts_dir"),
            "config file should not contain artifacts_dir when not provided"
        );
    }

    #[test]
    fn state_file_records_version_and_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("manifest.toml");
        fs::write(&manifest_path, valid_manifest_toml()).unwrap();

        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();

        run(&working, &manifest_path, None, None).unwrap();

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
        let manifest_path = dir.path().join("manifest.toml");
        fs::write(&manifest_path, valid_manifest_toml()).unwrap();

        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();

        run(&working, &manifest_path, None, None).unwrap();

        let state_content = fs::read_to_string(working.join(".runa").join("state.toml")).unwrap();
        assert!(
            !state_content.contains("methodology_path"),
            "state file should not contain methodology_path"
        );
    }

    #[test]
    fn custom_config_path_writes_config_there() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("manifest.toml");
        fs::write(&manifest_path, valid_manifest_toml()).unwrap();

        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();

        let custom_config = dir.path().join("custom").join("config.toml");
        run(&working, &manifest_path, None, Some(&custom_config)).unwrap();

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

        let err = run(dir.path(), &bogus, None, None).unwrap_err();
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

        let err = run(dir.path(), &manifest_path, None, None).unwrap_err();
        assert!(
            matches!(err, InitError::ManifestInvalid(_)),
            "expected ManifestInvalid, got: {err}"
        );
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
        let manifest_path = dir.path().join("manifest.toml");
        fs::write(&manifest_path, valid_manifest_toml()).unwrap();

        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();

        let summary1 = run(&working, &manifest_path, None, None).unwrap();
        let summary2 = run(&working, &manifest_path, None, None).unwrap();

        assert_eq!(summary1.methodology_name, summary2.methodology_name);
        assert_eq!(summary1.artifact_type_count, summary2.artifact_type_count);
        assert_eq!(summary1.protocol_count, summary2.protocol_count);
    }
}
