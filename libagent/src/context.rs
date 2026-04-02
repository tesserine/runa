//! Agent-facing context injection construction and prompt rendering.
//!
//! Converts a ready [`ProtocolDeclaration`] plus the
//! current [`ArtifactStore`] into the stable context
//! delivered to agents during `runa step`. [`build_context`] gathers the
//! artifacts; [`render_context_prompt`] turns the context into prose.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ArtifactStore, ProtocolDeclaration, ValidationStatus};

/// Whether an artifact is a hard dependency (`Requires`) or optional input (`Accepts`)
/// for the protocol being invoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactRelationship {
    Requires,
    Accepts,
}

/// A resolved reference to a valid artifact instance available to the agent.
///
/// Contains the internal filesystem `path` for reopening the artifact's content.
/// For serialization contexts that must not expose internal paths, convert to
/// [`ArtifactRefView`] instead.
#[derive(Debug, Clone, PartialEq)]
pub struct ArtifactRef {
    pub artifact_type: String,
    pub instance_id: String,
    pub path: PathBuf,
    pub display_path: String,
    pub content_hash: String,
    pub relationship: ArtifactRelationship,
}

/// Serialization-safe view of [`ArtifactRef`] that exposes `display_path`
/// but omits the internal filesystem `path`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ArtifactRefView {
    pub artifact_type: String,
    pub instance_id: String,
    pub display_path: String,
    pub content_hash: String,
    pub relationship: ArtifactRelationship,
}

/// Artifact type names the agent is expected to produce for this protocol invocation.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExpectedOutputs {
    /// Artifact types that must be delivered -- postconditions fail if missing.
    pub produces: Vec<String>,
    /// Artifact types that may optionally be delivered -- validated if present.
    pub may_produce: Vec<String>,
}

/// The complete agent-facing context for one protocol invocation.
///
/// Contains the protocol name, optional work unit scope, protocol instructions,
/// all valid required and accepted artifact references, and the expected output
/// types. Built by [`build_context`] and rendered to prose by
/// [`render_context_prompt`].
#[derive(Debug, Clone, PartialEq)]
pub struct ContextInjection {
    pub protocol: String,
    pub work_unit: Option<String>,
    pub instructions: String,
    pub inputs: Vec<ArtifactRef>,
    pub expected_outputs: ExpectedOutputs,
}

/// Serialization-safe view of [`ContextInjection`] that uses
/// [`ArtifactRefView`] instead of [`ArtifactRef`], omitting internal paths.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContextInjectionView {
    pub protocol: String,
    pub work_unit: Option<String>,
    pub instructions: String,
    pub inputs: Vec<ArtifactRefView>,
    pub expected_outputs: ExpectedOutputs,
}

/// Build the agent-facing context for a single protocol invocation.
///
/// Gathers all valid `requires` and `accepts` artifact instances from the store
/// (scoped by `work_unit`) into ordered [`ArtifactRef`] entries, paired with the
/// protocol's instructions and expected output types.
pub fn build_context(
    protocol: &ProtocolDeclaration,
    store: &ArtifactStore,
    work_unit: Option<&str>,
) -> ContextInjection {
    let mut inputs = Vec::new();

    collect_inputs(
        &mut inputs,
        store,
        &protocol.requires,
        ArtifactRelationship::Requires,
        work_unit,
    );
    collect_inputs(
        &mut inputs,
        store,
        &protocol.accepts,
        ArtifactRelationship::Accepts,
        work_unit,
    );

    ContextInjection {
        protocol: protocol.name.clone(),
        work_unit: work_unit.map(str::to_owned),
        instructions: protocol.instructions.clone().unwrap_or_default(),
        inputs,
        expected_outputs: ExpectedOutputs {
            produces: protocol.produces.clone(),
            may_produce: protocol.may_produce.clone(),
        },
    }
}

/// Render the agent-facing context as natural language prose.
///
/// Reads artifact files from the paths in `ContextInjection`, parses them as
/// JSON, and transforms them to readable prose. Errors reading or parsing
/// individual artifacts are rendered inline rather than failing the prompt.
pub fn render_context_prompt(context: &ContextInjection) -> String {
    let mut sections = Vec::new();
    sections.push(match &context.work_unit {
        Some(work_unit) => format!("# Protocol: {} (work_unit={work_unit})", context.protocol),
        None => format!("# Protocol: {}", context.protocol),
    });

    if !context.instructions.is_empty() {
        sections.push("\n## Protocol instructions".to_string());
        sections.push(context.instructions.clone());
    }

    let required: Vec<_> = context
        .inputs
        .iter()
        .filter(|input| input.relationship == ArtifactRelationship::Requires)
        .collect();
    if !required.is_empty() {
        sections.push("\n## What you've been given".to_string());
        for input in required {
            sections.push(render_input(input));
        }
    }

    let available: Vec<_> = context
        .inputs
        .iter()
        .filter(|input| input.relationship == ArtifactRelationship::Accepts)
        .collect();
    if !available.is_empty() {
        sections.push("\n## Additional context".to_string());
        for input in available {
            sections.push(render_input(input));
        }
    }

    sections.push("\n## What you need to deliver".to_string());
    if !context.expected_outputs.produces.is_empty() {
        let list = context.expected_outputs.produces.join(", ");
        sections.push(format!("\nYou must produce: {list}"));
    }
    if !context.expected_outputs.may_produce.is_empty() {
        let list = context.expected_outputs.may_produce.join(", ");
        sections.push(format!("You may also produce: {list}"));
    }
    sections.push(
        "\nTo deliver each required output, call the tool with the matching name \
         and fill in the required fields."
            .to_string(),
    );

    sections.join("\n")
}

fn collect_inputs(
    target: &mut Vec<ArtifactRef>,
    store: &ArtifactStore,
    artifact_types: &[String],
    relationship: ArtifactRelationship,
    work_unit: Option<&str>,
) {
    for artifact_type in artifact_types {
        for (instance_id, state) in store.instances_of(artifact_type, work_unit) {
            if matches!(state.status, ValidationStatus::Valid) {
                target.push(ArtifactRef {
                    artifact_type: artifact_type.clone(),
                    instance_id: instance_id.to_string(),
                    path: state.path.clone(),
                    display_path: display_path(&state.path),
                    content_hash: state.content_hash.clone(),
                    relationship,
                });
            }
        }
    }
}

fn display_path(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

fn render_input(input: &ArtifactRef) -> String {
    let mut result = format!("\n### {} — {}", input.artifact_type, input.instance_id);

    match std::fs::read_to_string(&input.path) {
        Ok(content) => match serde_json::from_str::<Value>(&content) {
            Ok(value) => {
                result.push('\n');
                result.push_str(&json_to_prose(&value, 0));
            }
            Err(error) => {
                result.push_str(&format!("\n(Could not parse artifact: {error})"));
            }
        },
        Err(error) => {
            result.push_str(&format!("\n(Could not read artifact: {error})"));
        }
    }

    result
}

/// Convert a JSON value to human-readable prose.
pub fn json_to_prose(value: &Value, indent: usize) -> String {
    let prefix = "  ".repeat(indent);
    match value {
        Value::Object(map) => {
            let mut lines = Vec::new();
            for (key, val) in map {
                let label = humanize_key(key);
                match val {
                    Value::Object(_) | Value::Array(_) => {
                        lines.push(format!("{prefix}**{label}:**"));
                        lines.push(json_to_prose(val, indent + 1));
                    }
                    _ => lines.push(format!("{prefix}**{label}:** {}", inline_value(val))),
                }
            }
            lines.join("\n")
        }
        Value::Array(arr) => {
            let mut lines = Vec::new();
            for (index, item) in arr.iter().enumerate() {
                let num = index + 1;
                match item {
                    Value::Object(_) | Value::Array(_) => {
                        lines.push(format!("{prefix}{num}."));
                        lines.push(json_to_prose(item, indent + 1));
                    }
                    _ => lines.push(format!("{prefix}{num}. {}", inline_value(item))),
                }
            }
            lines.join("\n")
        }
        other => format!("{prefix}{}", inline_value(other)),
    }
}

fn inline_value(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        _ => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn humanize_key(key: &str) -> String {
    let mut result = key.replace('_', " ");
    if let Some(first) = result.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    result
}

impl From<&ArtifactRef> for ArtifactRefView {
    fn from(input: &ArtifactRef) -> Self {
        Self {
            artifact_type: input.artifact_type.clone(),
            instance_id: input.instance_id.clone(),
            display_path: input.display_path.clone(),
            content_hash: input.content_hash.clone(),
            relationship: input.relationship,
        }
    }
}

impl From<&ContextInjection> for ContextInjectionView {
    fn from(context: &ContextInjection) -> Self {
        Self {
            protocol: context.protocol.clone(),
            work_unit: context.work_unit.clone(),
            instructions: context.instructions.clone(),
            inputs: context.inputs.iter().map(ArtifactRefView::from).collect(),
            expected_outputs: context.expected_outputs.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::make_store;
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn build_context_collects_required_and_accepted_inputs() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = make_store(&tmp.path().join("store"), vec!["constraints", "notes"]);
        let constraints_path = tmp.path().join("workspace/constraints/spec.json");
        let notes_path = tmp.path().join("workspace/notes/notes.json");

        store
            .record(
                "constraints",
                "spec",
                &constraints_path,
                &json!({"title": "ship step"}),
            )
            .unwrap();
        store
            .record("notes", "notes", &notes_path, &json!({"title": "keep"}))
            .unwrap();

        let protocol = ProtocolDeclaration {
            name: "implement".into(),
            requires: vec!["constraints".into()],
            accepts: vec!["notes".into()],
            produces: vec!["implementation".into()],
            may_produce: vec!["scratchpad".into()],
            scoped: false,
            trigger: crate::TriggerCondition::OnArtifact {
                name: "constraints".into(),
            },
            instructions: Some("# implement\n".into()),
        };

        let context = build_context(&protocol, &store, None);
        assert_eq!(context.protocol, "implement");
        assert_eq!(context.work_unit, None);
        assert_eq!(context.instructions, "# implement\n");
        assert_eq!(
            context.expected_outputs,
            ExpectedOutputs {
                produces: vec!["implementation".into()],
                may_produce: vec!["scratchpad".into()],
            }
        );
        assert_eq!(context.inputs.len(), 2);
        assert_eq!(
            context.inputs[0],
            ArtifactRef {
                artifact_type: "constraints".into(),
                instance_id: "spec".into(),
                path: constraints_path.clone(),
                display_path: constraints_path.display().to_string(),
                content_hash:
                    "sha256:dd4077b358533c789242e86ac7f5e7dffa0a587d5b4acfd343c612ae9ddfd315".into(),
                relationship: ArtifactRelationship::Requires,
            }
        );
        assert_eq!(
            context.inputs[1],
            ArtifactRef {
                artifact_type: "notes".into(),
                instance_id: "notes".into(),
                path: notes_path.clone(),
                display_path: notes_path.display().to_string(),
                content_hash:
                    "sha256:c623bcb14b09ea83fe711bfb893ea3d56f13a23b95bad47e04c4dec264267abd".into(),
                relationship: ArtifactRelationship::Accepts,
            }
        );
    }

    #[test]
    fn build_context_omits_missing_and_invalid_accepted_inputs() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = make_store(&tmp.path().join("store"), vec!["constraints", "notes"]);
        store
            .record(
                "constraints",
                "spec",
                Path::new("constraints/spec.json"),
                &json!({"title": "ship step"}),
            )
            .unwrap();
        store
            .record(
                "notes",
                "bad",
                Path::new("notes/bad.json"),
                &json!({"bad": true}),
            )
            .unwrap();

        let protocol = ProtocolDeclaration {
            name: "implement".into(),
            requires: vec!["constraints".into()],
            accepts: vec!["notes".into(), "missing".into()],
            produces: Vec::new(),
            may_produce: Vec::new(),
            scoped: false,
            trigger: crate::TriggerCondition::OnArtifact {
                name: "constraints".into(),
            },
            instructions: None,
        };

        let context = build_context(&protocol, &store, None);
        assert_eq!(context.inputs.len(), 1);
        assert_eq!(context.work_unit, None);
        assert_eq!(context.instructions, "");
        assert_eq!(context.inputs[0].artifact_type, "constraints");
        assert_eq!(
            context.inputs[0].relationship,
            ArtifactRelationship::Requires
        );
    }

    #[cfg(unix)]
    #[test]
    fn display_path_converts_non_utf8_paths_lossily() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let path = std::path::Path::new(OsStr::from_bytes(b"workspace/constraints/spec-\xFF.json"));

        let display = display_path(path);
        assert_eq!(display, "workspace/constraints/spec-\u{FFFD}.json");
        let artifact = ArtifactRef {
            artifact_type: "constraints".into(),
            instance_id: "spec".into(),
            path: path.to_path_buf(),
            display_path: display,
            content_hash: "sha256:test".into(),
            relationship: ArtifactRelationship::Requires,
        };
        let view = ArtifactRefView::from(&artifact);
        assert!(serde_json::to_string(&view).is_ok());
    }

    #[test]
    fn context_view_uses_display_path_only() {
        let context = ContextInjection {
            protocol: "implement".into(),
            work_unit: None,
            instructions: String::new(),
            inputs: vec![ArtifactRef {
                artifact_type: "constraints".into(),
                instance_id: "spec".into(),
                path: PathBuf::from("/tmp/spec.json"),
                display_path: "/tmp/spec.json".into(),
                content_hash: "sha256:test".into(),
                relationship: ArtifactRelationship::Requires,
            }],
            expected_outputs: ExpectedOutputs {
                produces: vec!["implementation".into()],
                may_produce: Vec::new(),
            },
        };

        let value = serde_json::to_value(ContextInjectionView::from(&context)).unwrap();
        assert_eq!(value["inputs"][0]["display_path"], "/tmp/spec.json");
        assert!(value["inputs"][0].get("path").is_none(), "{value:#}");
    }

    #[test]
    fn build_context_scoped() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = make_store(&tmp.path().join("store"), vec!["constraints"]);

        // WU-A artifact.
        store
            .record(
                "constraints",
                "a1",
                Path::new("constraints/a1.json"),
                &json!({"title": "A", "work_unit": "wu-a"}),
            )
            .unwrap();
        // WU-B artifact.
        store
            .record(
                "constraints",
                "b1",
                Path::new("constraints/b1.json"),
                &json!({"title": "B", "work_unit": "wu-b"}),
            )
            .unwrap();
        // Unpartitioned artifact.
        store
            .record(
                "constraints",
                "shared",
                Path::new("constraints/shared.json"),
                &json!({"title": "shared"}),
            )
            .unwrap();

        let protocol = ProtocolDeclaration {
            name: "implement".into(),
            requires: vec!["constraints".into()],
            accepts: Vec::new(),
            produces: Vec::new(),
            may_produce: Vec::new(),
            scoped: false,
            trigger: crate::TriggerCondition::OnArtifact {
                name: "constraints".into(),
            },
            instructions: None,
        };

        // Scoped to WU-A: sees a1 + shared, not b1.
        let context = build_context(&protocol, &store, Some("wu-a"));
        assert_eq!(context.work_unit.as_deref(), Some("wu-a"));
        let ids: Vec<&str> = context
            .inputs
            .iter()
            .map(|i| i.instance_id.as_str())
            .collect();
        assert_eq!(ids, vec!["a1", "shared"]);
    }

    #[test]
    fn humanize_key_snake_case() {
        assert_eq!(humanize_key("work_unit"), "Work unit");
        assert_eq!(humanize_key("acceptance_criteria"), "Acceptance criteria");
        assert_eq!(humanize_key("title"), "Title");
    }

    #[test]
    fn json_to_prose_formats_nested_values() {
        let val = json!({
            "title": "Test",
            "details": {
                "author": "agent",
                "priority": "high"
            },
            "scenarios": [
                {"name": "reject-missing-field", "criterion": "returns 400"}
            ]
        });

        let prose = json_to_prose(&val, 0);
        assert!(prose.contains("**Title:** Test"));
        assert!(prose.contains("**Details:**"));
        assert!(prose.contains("  **Author:** agent"));
        assert!(prose.contains("**Scenarios:**"));
        assert!(prose.contains("1."));
        assert!(prose.contains("**Criterion:** returns 400"));
    }

    #[test]
    fn render_context_prompt_includes_sections_and_tool_guidance() {
        let prompt = render_context_prompt(&ContextInjection {
            protocol: "implement".into(),
            work_unit: None,
            instructions: "# Follow the protocol\n".into(),
            inputs: Vec::new(),
            expected_outputs: ExpectedOutputs {
                produces: vec!["implementation".into()],
                may_produce: vec!["notes".into()],
            },
        });

        assert!(prompt.contains("# Protocol: implement"));
        assert!(prompt.contains("## Protocol instructions"));
        assert!(prompt.contains("# Follow the protocol"));
        assert!(prompt.contains("You must produce: implementation"));
        assert!(prompt.contains("You may also produce: notes"));
        assert!(prompt.contains(
            "To deliver each required output, call the tool with the matching name and fill in the required fields."
        ));
    }

    #[test]
    fn render_context_prompt_includes_work_unit_in_heading() {
        let prompt = render_context_prompt(&ContextInjection {
            protocol: "implement".into(),
            work_unit: Some("wu-a".into()),
            instructions: String::new(),
            inputs: Vec::new(),
            expected_outputs: ExpectedOutputs {
                produces: vec!["implementation".into()],
                may_produce: Vec::new(),
            },
        });

        assert!(prompt.contains("# Protocol: implement (work_unit=wu-a)"));
    }

    #[test]
    fn render_context_prompt_inlines_artifact_read_errors() {
        let prompt = render_context_prompt(&ContextInjection {
            protocol: "implement".into(),
            work_unit: None,
            instructions: String::new(),
            inputs: vec![ArtifactRef {
                artifact_type: "constraints".into(),
                instance_id: "missing".into(),
                path: PathBuf::from("/tmp/does-not-exist.json"),
                display_path: "/tmp/does-not-exist.json".into(),
                content_hash: "sha256:test".into(),
                relationship: ArtifactRelationship::Requires,
            }],
            expected_outputs: ExpectedOutputs {
                produces: vec!["implementation".into()],
                may_produce: Vec::new(),
            },
        });

        assert!(prompt.contains("## What you've been given"));
        assert!(prompt.contains("### constraints — missing"));
        assert!(prompt.contains("Could not read artifact"));
    }
}
