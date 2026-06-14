use std::fs;
use std::path::{Path, PathBuf};

/// Write a methodology layout in `dir`: manifest TOML, schema files, and
/// protocol instruction files. Returns the manifest file path.
///
/// Duplicated from `libagent/src/test_helpers.rs` — CLI integration tests
/// cannot access that helper (`#[cfg(test)]` internal). Keep both in sync.
pub fn write_methodology(
    dir: &Path,
    manifest_toml: &str,
    schemas: &[(&str, &str)],
    protocols: &[&str],
) -> PathBuf {
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

#[allow(dead_code)]
pub fn scoped_work_unit_manifest_toml() -> &'static str {
    r#"
name = "groundwork"

[[artifact_types]]
name = "work-unit"

[[artifact_types]]
name = "claim"

[[protocols]]
name = "take"
requires = ["work-unit"]
produces = ["claim"]
scoped = true
trigger = { type = "on_artifact", name = "work-unit" }
"#
}

#[allow(dead_code)]
pub fn scoped_work_unit_schemas() -> &'static [(&'static str, &'static str)] {
    &[
        (
            "work-unit",
            r#"{"type":"object","required":["title","description","acceptance_criteria"],"properties":{"title":{"type":"string"},"description":{"type":"string"},"acceptance_criteria":{"type":"array","items":{"type":"string"}},"handle":{"type":"object"}}}"#,
        ),
        (
            "claim",
            r#"{"type":"object","required":["work_unit","scope"],"properties":{"work_unit":{"type":"string"},"scope":{"type":"string"}}}"#,
        ),
    ]
}

#[allow(dead_code)]
pub fn github_work_unit_json(number: u64) -> String {
    format!(
        r#"{{"title":"Scope","description":"Enforce canonical scope","acceptance_criteria":["Reject aliases"],"handle":{{"forge_tag":"github","url":"https://github.com/tesserine/runa/issues/{number}","number":{number}}}}}"#
    )
}

#[allow(dead_code)]
pub fn append_github_forge_config(project_dir: &Path, owner: &str, name: &str) {
    fs::write(
        project_dir.join(".runa/project.toml"),
        format!(
            "{}\n[target_project]\nforge_type = \"github\"\n\n[[target_project.repositories]]\nselector = \"{name}\"\nowner = \"{owner}\"\nname = \"{name}\"\n",
            fs::read_to_string(project_dir.join(".runa/project.toml")).unwrap()
        ),
    )
    .unwrap();
}
