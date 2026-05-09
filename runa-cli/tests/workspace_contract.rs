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

fn string_array(values: &[&str]) -> Vec<toml::Value> {
    values
        .iter()
        .map(|value| toml::Value::String((*value).to_string()))
        .collect()
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
        Some(&string_array(&["main"])),
        "release should be allowed only from main"
    );
    assert_eq!(config["release"].as_bool(), Some(true));
    assert_eq!(config["shared-version"].as_bool(), Some(true));
    assert_eq!(config["dependent-version"].as_str(), Some("fix"));
    assert_eq!(config["consolidate-commits"].as_bool(), Some(true));
    assert_eq!(
        config["pre-release-replacements"]
            .as_array()
            .map(Vec::as_slice),
        Some(&[][..])
    );
    assert_eq!(
        config["pre-release-hook"].as_array(),
        Some(&string_array(&["true"]))
    );
    assert_eq!(
        config["pre-release-commit-message"].as_str(),
        Some("chore(release): bump workspace version to {{version}}")
    );
    assert_eq!(config["tag"].as_bool(), Some(true));
    assert_eq!(config["tag-name"].as_str(), Some("v{{version}}"));
    assert_eq!(config["tag-message"].as_str(), Some("Release {{tag_name}}"));
    assert_eq!(config["sign-commit"].as_bool(), Some(false));
    assert_eq!(config["sign-tag"].as_bool(), Some(false));
    assert_eq!(config["push"].as_bool(), Some(true));
    assert_eq!(config["push-remote"].as_str(), Some("origin"));
    assert_eq!(
        config["push-options"].as_array().map(Vec::as_slice),
        Some(&[][..])
    );
    assert_eq!(config["publish"].as_bool(), Some(false));
    assert_eq!(
        config["owners"].as_array().map(Vec::as_slice),
        Some(&[][..])
    );
    assert_eq!(config["verify"].as_bool(), Some(true));
    assert_eq!(
        config["enable-features"].as_array().map(Vec::as_slice),
        Some(&[][..])
    );
    assert_eq!(config["enable-all-features"].as_bool(), Some(false));
    assert_eq!(config["metadata"].as_str(), Some("optional"));
    assert_eq!(config["certs-source"].as_str(), Some("webpki"));
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
    assert_eq!(
        replacement.get("prerelease").and_then(toml::Value::as_bool),
        Some(true),
        "CHANGELOG.md replacement should run during RC releases"
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
        script.contains("source \"$workspace_root/scripts/release-check\""),
        "release verification should source release-check instead of defining a second parser"
    );
    assert!(
        script.contains("cargo release patch --execute --no-confirm"),
        "release verification should exercise the stable cargo-release operation"
    );
    assert!(
        script.contains("cargo release rc --execute --no-confirm"),
        "release verification should exercise the RC cargo-release operation"
    );
    assert!(
        script.contains("uncommitted changes detected"),
        "release verification should assert dirty-tree refusal"
    );
    assert!(
        script.contains("hostile_home=\"$scratch/hostile-home\""),
        "release verification should create an isolated hostile cargo-release home"
    );
    assert!(
        script.contains("pre-release-hook = [\"/bin/false\"]"),
        "release verification should model hostile hook inheritance"
    );
    assert!(
        script.contains("cargo release config"),
        "release verification should inspect resolved cargo-release config"
    );
    assert!(
        script.contains("pre-release-hook = [\"true\"]"),
        "release verification should assert the workspace-pinned no-op hook"
    );
    assert!(
        script.contains("target/release/runa\" --version"),
        "release verification should check the runa binary version"
    );
    assert!(
        script.contains("target/release/runa-mcp\" --version"),
        "release verification should check the runa-mcp binary version"
    );
    assert!(
        script.contains("./scripts/release-check release \"$tag_name\""),
        "release verification should prove produced tags pass release-check release"
    );
    assert!(
        !script.contains("sed -n '/^\\[workspace.package\\]/"),
        "release verification should not parse workspace versions with its own sed expression"
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

fn release_workflow_tag_patterns() -> Vec<String> {
    let workflow = read_workspace_file(".github/workflows/release.yml");
    let mut patterns = Vec::new();
    let mut in_tags = false;

    for line in workflow.lines() {
        if line == "    tags:" {
            in_tags = true;
            continue;
        }

        if in_tags {
            if !line.starts_with("      - ") {
                break;
            }
            patterns.push(
                line.trim()
                    .trim_start_matches("- ")
                    .trim_matches('"')
                    .to_string(),
            );
        }
    }

    patterns
}

fn documented_actions_pattern_matches(pattern: &str, candidate: &str) -> bool {
    let pattern = pattern.as_bytes();
    let candidate = candidate.as_bytes();
    let mut pattern_index = 0;
    let mut candidate_index = 0;

    while pattern_index < pattern.len() {
        if pattern[pattern_index] == b'[' {
            let range_end = pattern[pattern_index..]
                .iter()
                .position(|byte| *byte == b']')
                .map(|offset| pattern_index + offset)
                .expect("test pattern character class should close");
            assert_eq!(
                &pattern[pattern_index..=range_end],
                b"[0-9]",
                "test matcher only models the documented digit class used by release tags"
            );
            let one_or_more = pattern.get(range_end + 1) == Some(&b'+');
            let start = candidate_index;
            while candidate
                .get(candidate_index)
                .is_some_and(u8::is_ascii_digit)
            {
                candidate_index += 1;
            }
            if one_or_more {
                if candidate_index == start {
                    return false;
                }
                pattern_index = range_end + 2;
            } else {
                if candidate_index != start + 1 {
                    return false;
                }
                pattern_index = range_end + 1;
            }
            continue;
        }

        if candidate.get(candidate_index) != pattern.get(pattern_index) {
            return false;
        }
        pattern_index += 1;
        candidate_index += 1;
    }

    candidate_index == candidate.len()
}

fn matches_any_documented_actions_pattern(patterns: &[String], candidate: &str) -> bool {
    patterns
        .iter()
        .any(|pattern| documented_actions_pattern_matches(pattern, candidate))
}

#[test]
fn release_check_script_is_the_release_verifier_surface() {
    let script_path = workspace_root().join("scripts/release-check");
    let script = std::fs::read_to_string(&script_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", script_path.display()));

    assert!(
        script.contains("scripts/release-check metadata")
            && script.contains("scripts/release-check notes vX.Y.Z[-rc.N]")
            && script.contains("--runa-bin PATH")
            && script.contains("--runa-mcp-bin PATH"),
        "release-check should expose metadata, notes, and release modes for runa binaries"
    );
    assert!(
        !script.contains("--container-image") && !script.contains("--base-image"),
        "runa release-check should not validate container or base image surfaces"
    );
}

#[test]
fn github_release_workflow_triggers_only_for_documented_tag_shapes() {
    let patterns = release_workflow_tag_patterns();

    assert!(
        !patterns.iter().any(|pattern| pattern == "v*.*.*"),
        "release workflow should not trigger on arbitrary v*.*.* tags"
    );
    assert_eq!(
        patterns,
        ["v[0-9]+.[0-9]+.[0-9]+", "v[0-9]+.[0-9]+.[0-9]+-rc.[0-9]+"],
        "release workflow should trigger only documented stable and RC tag shapes"
    );
}

#[test]
fn github_release_workflow_tag_filters_match_the_documented_release_contract() {
    let patterns = release_workflow_tag_patterns();

    for accepted in ["v0.1.2", "v10.20.300", "v0.1.2-rc.1", "v10.20.300-rc.400"] {
        assert!(
            matches_any_documented_actions_pattern(&patterns, accepted),
            "release workflow tag filters should match documented tag {accepted}"
        );
    }

    for rejected in [
        "v0.1",
        "v0.1.2.3",
        "v0.1.2-beta.1",
        "v0.1.2-rc",
        "v0.1.2-rc.x",
        "runa-v0.1.2",
    ] {
        assert!(
            !matches_any_documented_actions_pattern(&patterns, rejected),
            "release workflow tag filters should reject undocumented tag {rejected}"
        );
    }
}

#[test]
fn github_release_publication_is_not_coupled_to_path_filters() {
    let workflow = read_workspace_file(".github/workflows/release.yml");

    assert!(
        workflow.contains("    tags:"),
        "release publication workflow should be triggered by release tags"
    );
    assert!(
        !workflow.contains("    paths:"),
        "release publication workflow should not path-filter tag pushes"
    );
}

#[test]
fn release_metadata_workflow_keeps_path_filtered_branch_and_pr_checks() {
    let workflow = read_workspace_file(".github/workflows/release-metadata.yml");

    assert!(
        workflow.contains("name: Release Metadata"),
        "release metadata workflow should exist separately from tag publication"
    );
    assert!(
        workflow.contains("  push:") && workflow.contains("    branches: [main]"),
        "release metadata workflow should run on main branch pushes"
    );
    assert!(
        workflow.contains("  pull_request:") && workflow.contains("    paths:"),
        "release metadata workflow should retain PR path filtering"
    );
    assert!(
        workflow.contains("./scripts/release-check metadata"),
        "release metadata workflow should run release-check metadata"
    );
}

#[test]
fn github_release_workflow_marks_only_rc_tags_as_prereleases() {
    let workflow = read_workspace_file(".github/workflows/release.yml");

    assert!(
        workflow.contains("^v[0-9]+[.][0-9]+[.][0-9]+-rc[.][0-9]+$"),
        "release workflow should mark only documented RC tags as GitHub prereleases"
    );
    assert!(
        !workflow.contains("[[ \"$GITHUB_REF_NAME\" == *-* ]]"),
        "release workflow should not treat every hyphenated tag as a prerelease"
    );
}

#[test]
fn release_documentation_describes_runa_release_identity() {
    let releasing = read_workspace_file("RELEASING.md");

    assert!(releasing.contains("runa --version"));
    assert!(releasing.contains("runa-mcp --version"));
    assert!(
        releasing.contains("runa's own release check does not validate base images"),
        "RELEASING.md should name the runa/base release boundary"
    );
    assert!(
        releasing.contains("Only `vX.Y.Z-rc.N` tags are published as GitHub prereleases."),
        "RELEASING.md should describe prerelease publication with RC precision"
    );
}
