use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

const RUNA_DIR: &str = ".runa";
const STATE_FILENAME: &str = "state.toml";

#[derive(Debug)]
pub struct InitSummary {
    pub methodology_name: String,
    pub artifact_type_count: usize,
    pub skill_count: usize,
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

#[derive(Serialize)]
struct State {
    methodology_path: String,
    methodology_name: String,
}

pub fn run(working_dir: &Path, methodology: &Path) -> Result<InitSummary, InitError> {
    if !methodology.exists() {
        return Err(InitError::MethodologyNotFound {
            path: methodology.to_path_buf(),
        });
    }

    let manifest =
        libagent::manifest::parse(methodology).map_err(InitError::ManifestInvalid)?;

    let canonical_path = fs::canonicalize(methodology).map_err(InitError::Io)?;

    let runa_dir = working_dir.join(RUNA_DIR);
    fs::create_dir_all(&runa_dir).map_err(InitError::Io)?;

    let state = State {
        methodology_path: canonical_path.display().to_string(),
        methodology_name: manifest.name.clone(),
    };
    let state_toml = toml::to_string(&state).expect("State serialization should not fail");
    fs::write(runa_dir.join(STATE_FILENAME), state_toml).map_err(InitError::Io)?;

    Ok(InitSummary {
        methodology_name: manifest.name,
        artifact_type_count: manifest.artifact_types.len(),
        skill_count: manifest.skills.len(),
    })
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

[[skills]]
name = "ground"
produces = ["constraints"]
trigger = { type = "on_signal", name = "init" }

[[skills]]
name = "design"
requires = ["constraints"]
produces = ["design-doc"]
trigger = { type = "on_artifact", name = "constraints" }

[[skills]]
name = "review"
requires = ["design-doc"]
trigger = { type = "on_artifact", name = "design-doc" }
"#
    }

    #[test]
    fn valid_manifest_creates_runa_dir_and_state_file() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("manifest.toml");
        fs::write(&manifest_path, valid_manifest_toml()).unwrap();

        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();

        let summary = run(&working, &manifest_path).unwrap();

        assert_eq!(summary.methodology_name, "groundwork");
        assert_eq!(summary.artifact_type_count, 2);
        assert_eq!(summary.skill_count, 3);

        let runa_dir = working.join(".runa");
        assert!(runa_dir.is_dir());

        let state_path = runa_dir.join("state.toml");
        assert!(state_path.is_file());
    }

    #[test]
    fn nonexistent_methodology_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let bogus = dir.path().join("no-such-file.toml");

        let err = run(dir.path(), &bogus).unwrap_err();
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

        let err = run(dir.path(), &manifest_path).unwrap_err();
        assert!(
            matches!(err, InitError::ManifestInvalid(_)),
            "expected ManifestInvalid, got: {err}"
        );
    }

    #[test]
    fn idempotent_run_succeeds_twice() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("manifest.toml");
        fs::write(&manifest_path, valid_manifest_toml()).unwrap();

        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();

        let summary1 = run(&working, &manifest_path).unwrap();
        let summary2 = run(&working, &manifest_path).unwrap();

        assert_eq!(summary1.methodology_name, summary2.methodology_name);
        assert_eq!(summary1.artifact_type_count, summary2.artifact_type_count);
        assert_eq!(summary1.skill_count, summary2.skill_count);
    }

    #[test]
    fn state_file_records_methodology_path_and_name() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("manifest.toml");
        fs::write(&manifest_path, valid_manifest_toml()).unwrap();

        let working = dir.path().join("project");
        fs::create_dir(&working).unwrap();

        run(&working, &manifest_path).unwrap();

        let state_content =
            fs::read_to_string(working.join(".runa").join("state.toml")).unwrap();
        assert!(
            state_content.contains("groundwork"),
            "state file should contain methodology name"
        );
        let canonical = fs::canonicalize(&manifest_path).unwrap();
        assert!(
            state_content.contains(&canonical.display().to_string()),
            "state file should contain canonical methodology path"
        );
    }
}
