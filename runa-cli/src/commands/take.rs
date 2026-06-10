use std::fmt;
use std::path::Path;

use crate::project::{self, ProjectError};

#[derive(Debug)]
pub enum TakeCommandError {
    Project(ProjectError),
    Take(libagent::TakeError),
}

impl fmt::Display for TakeCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Project(err) => write!(f, "{err}"),
            Self::Take(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for TakeCommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Project(err) => Some(err),
            Self::Take(err) => Some(err),
        }
    }
}

impl From<ProjectError> for TakeCommandError {
    fn from(err: ProjectError) -> Self {
        Self::Project(err)
    }
}

impl From<libagent::TakeError> for TakeCommandError {
    fn from(err: libagent::TakeError) -> Self {
        Self::Take(err)
    }
}

pub fn run(
    working_dir: &Path,
    config_override: Option<&Path>,
    id: u64,
) -> Result<String, TakeCommandError> {
    let mut loaded = project::load(working_dir, config_override)?;
    Ok(libagent::take_work_unit(&mut loaded, id)?)
}

#[cfg(test)]
pub fn run_with_fetcher(
    working_dir: &Path,
    config_override: Option<&Path>,
    id: u64,
    fetcher: &impl libagent::ForgeFetcher,
    env: impl Fn(&str) -> Option<String>,
) -> Result<String, TakeCommandError> {
    let mut loaded = project::load(working_dir, config_override)?;
    Ok(libagent::take_work_unit_with_fetcher(
        &mut loaded,
        id,
        fetcher,
        env,
    )?)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fs;
    use std::path::Path;

    use libagent::{ForgeFetcher, ForgeHandle, ForgeIssue, TakeError};
    use serde_json::Value;

    use crate::project;

    #[test]
    fn run_with_fetcher_writes_work_unit_and_returns_id_for_stdout() {
        let dir = tempfile::tempdir().unwrap();
        let project_dir = initialized_project(dir.path());
        let fetcher = FakeFetcher {
            calls: RefCell::new(0),
        };

        let id = super::run_with_fetcher(&project_dir, None, 58, &fetcher, token_env).unwrap();

        assert_eq!(id, "58");
        assert_eq!(*fetcher.calls.borrow(), 1);
        let artifact_path = project_dir.join(".runa/workspace/work-unit/58.json");
        let artifact: Value = serde_json::from_str(&fs::read_to_string(artifact_path).unwrap())
            .expect("work-unit JSON");
        assert_eq!(artifact["title"], "Fetch work");
        assert_eq!(
            artifact["acceptance_criteria"],
            serde_json::json!(["Write it."])
        );
    }

    struct FakeFetcher {
        calls: RefCell<usize>,
    }

    impl ForgeFetcher for FakeFetcher {
        fn fetch_github_issue(
            &self,
            owner: &str,
            name: &str,
            id: u64,
            token: &str,
        ) -> Result<ForgeIssue, TakeError> {
            *self.calls.borrow_mut() += 1;
            assert_eq!(owner, "tesserine");
            assert_eq!(name, "runa");
            assert_eq!(id, 58);
            assert_eq!(token, "secret");
            Ok(ForgeIssue {
                title: "Fetch work".to_string(),
                body: "Verification gate:\n\n1. Write it.".to_string(),
                handle: ForgeHandle::Github {
                    url: "https://github.com/tesserine/runa/issues/58".to_string(),
                    number: 58,
                },
            })
        }

        fn fetch_sourcehut_issue(
            &self,
            _owner: &str,
            _name: &str,
            _tracker_id: u64,
            _id: u64,
            _token: &str,
        ) -> Result<ForgeIssue, TakeError> {
            unreachable!("github test should not fetch SourceHut")
        }
    }

    fn token_env(name: &str) -> Option<String> {
        (name == "GITHUB_TOKEN").then(|| "secret".to_string())
    }

    fn initialized_project(root: &Path) -> std::path::PathBuf {
        let manifest_path = crate::commands::take::tests::write_methodology(
            root,
            r#"
name = "groundwork"

[[artifact_types]]
name = "work-unit"

[[protocols]]
name = "take"
requires = ["work-unit"]
trigger = { type = "on_artifact", name = "work-unit" }
"#,
            &[(
                "work-unit",
                r#"{"type":"object","required":["title","description","acceptance_criteria"],"additionalProperties":false,"properties":{"title":{"type":"string","minLength":1},"description":{"type":"string","minLength":1},"acceptance_criteria":{"type":"array","minItems":1,"items":{"type":"string","minLength":1}},"handle":{"type":"object"}}}"#,
            )],
            &["take"],
        );
        let project_dir = root.join("project");
        fs::create_dir(&project_dir).unwrap();
        fs::create_dir(project_dir.join(".runa")).unwrap();
        let config = project::Config {
            methodology_path: fs::canonicalize(manifest_path)
                .unwrap()
                .display()
                .to_string(),
            logging: project::LoggingConfig::default(),
            agent: project::AgentConfig::default(),
            transcript: project::TranscriptConfig::default(),
            forge: project::ForgeConfig {
                forge_type: Some("github".to_string()),
                owner: Some("tesserine".to_string()),
                name: Some("runa".to_string()),
                tracker_id: None,
            },
        };
        fs::write(
            project_dir.join(".runa/config.toml"),
            toml::to_string(&config).unwrap(),
        )
        .unwrap();
        fs::write(
            project_dir.join(".runa/state.toml"),
            "initialized_at = \"2026-01-01T00:00:00Z\"\nruna_version = \"0.2.0\"\n",
        )
        .unwrap();
        project_dir
    }

    fn write_methodology(
        dir: &Path,
        manifest_toml: &str,
        schemas: &[(&str, &str)],
        protocols: &[&str],
    ) -> std::path::PathBuf {
        let manifest_path = dir.join("manifest.toml");
        fs::write(&manifest_path, manifest_toml).unwrap();

        let schemas_dir = dir.join("schemas");
        fs::create_dir_all(&schemas_dir).unwrap();
        for (name, content) in schemas {
            fs::write(schemas_dir.join(format!("{name}.schema.json")), content).unwrap();
        }

        for protocol_name in protocols {
            let protocol_dir = dir.join("protocols").join(protocol_name);
            fs::create_dir_all(&protocol_dir).unwrap();
            fs::write(
                protocol_dir.join("PROTOCOL.md"),
                format!("# {protocol_name}\n"),
            )
            .unwrap();
        }

        manifest_path
    }
}
