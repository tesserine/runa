use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

fn read_workspace_file(path: &str) -> String {
    std::fs::read_to_string(workspace_root().join(path))
        .unwrap_or_else(|error| panic!("failed to read {path}: {error}"))
}

fn read_workspace_toml(path: &str) -> toml::Value {
    read_workspace_file(path)
        .parse::<toml::Value>()
        .unwrap_or_else(|error| panic!("{path} should be valid TOML: {error}"))
}

#[test]
fn cargo_release_configuration_lives_at_the_workspace_root() {
    assert!(
        workspace_root().join("release.toml").is_file(),
        "release.toml should exist at the workspace root"
    );
}

#[test]
fn cargo_release_configuration_matches_the_shared_release_convention() {
    let config = read_workspace_toml("release.toml");

    assert_eq!(
        config.get("allow-branch").and_then(toml::Value::as_array),
        Some(&vec![toml::Value::String("main".to_string())]),
        "release should be allowed only from main"
    );
    assert_eq!(config["shared-version"].as_bool(), Some(true));
    assert_eq!(config["dependent-version"].as_str(), Some("fix"));
    assert_eq!(config["consolidate-commits"].as_bool(), Some(true));
    assert_eq!(config["tag-name"].as_str(), Some("v{{version}}"));
    assert_eq!(config["tag-message"].as_str(), Some("Release {{tag_name}}"));
    assert_eq!(config["push"].as_bool(), Some(true));
    assert_eq!(config["push-remote"].as_str(), Some("origin"));
    assert_eq!(config["publish"].as_bool(), Some(false));
}

#[test]
fn runa_cli_rolls_the_workspace_changelog_once_for_releases() {
    let manifest = read_workspace_toml("runa-cli/Cargo.toml");
    let replacement = manifest
        .get("package")
        .and_then(|package| package.get("metadata"))
        .and_then(|metadata| metadata.get("release"))
        .and_then(|release| release.get("pre-release-replacements"))
        .and_then(toml::Value::as_array)
        .and_then(|replacements| replacements.first())
        .and_then(toml::Value::as_table)
        .expect("runa-cli should declare one release changelog replacement");

    assert_eq!(
        replacement.get("file").and_then(toml::Value::as_str),
        Some("../CHANGELOG.md")
    );
    assert_eq!(
        replacement.get("search").and_then(toml::Value::as_str),
        Some("^## \\[Unreleased\\]")
    );
    assert_eq!(
        replacement.get("replace").and_then(toml::Value::as_str),
        Some("## [Unreleased]\n\n## [{{version}}] — {{date}}")
    );
    assert_eq!(
        replacement.get("exactly").and_then(toml::Value::as_integer),
        Some(1)
    );
}

#[test]
fn workspace_packages_inherit_the_workspace_version() {
    for manifest in [
        "libagent/Cargo.toml",
        "runa-cli/Cargo.toml",
        "runa-mcp/Cargo.toml",
    ] {
        let manifest_toml = read_workspace_toml(manifest);
        let version = manifest_toml
            .get("package")
            .and_then(|package| package.get("version"))
            .unwrap_or_else(|| panic!("{manifest} should declare package.version"));

        assert_eq!(
            version.get("workspace").and_then(toml::Value::as_bool),
            Some(true),
            "{manifest} should inherit version.workspace"
        );
    }
}

#[test]
fn release_adoption_verification_script_is_repo_tracked_operational_substrate() {
    let script_path = workspace_root().join("scripts/verify-release-adoption.sh");
    let script = std::fs::read_to_string(&script_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", script_path.display()));

    assert!(
        script.contains("git -C \"$source_repo\" -c core.excludesFile=/dev/null add ."),
        "release verification should seed the temp repo with global excludes disabled"
    );
    assert!(
        script.contains("cargo release patch --execute --no-confirm"),
        "release verification should exercise the real cargo-release operation"
    );
    assert!(
        script.contains("uncommitted changes detected"),
        "release verification should assert dirty-tree refusal"
    );
    assert!(
        script.contains("target/release/runa\" --version"),
        "release verification should check the runa binary version"
    );
    assert!(
        script.contains("target/release/runa-mcp\" --version"),
        "release verification should check the runa-mcp binary version"
    );

    #[cfg(unix)]
    {
        let mode = std::fs::metadata(&script_path)
            .unwrap_or_else(|error| panic!("failed to stat {}: {error}", script_path.display()))
            .permissions()
            .mode();
        assert_ne!(
            mode & 0o111,
            0,
            "{} should be executable",
            script_path.display()
        );
    }
}
