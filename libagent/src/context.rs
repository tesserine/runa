use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{ArtifactStore, SkillDeclaration, ValidationStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactRelationship {
    Requires,
    Accepts,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub artifact_type: String,
    pub instance_id: String,
    pub path: PathBuf,
    pub content_hash: String,
    pub relationship: ArtifactRelationship,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExpectedOutputs {
    pub produces: Vec<String>,
    pub may_produce: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextInjection {
    pub skill: String,
    pub inputs: Vec<ArtifactRef>,
    pub expected_outputs: ExpectedOutputs,
}

pub fn build_context(skill: &SkillDeclaration, store: &ArtifactStore) -> ContextInjection {
    let mut inputs = Vec::new();

    collect_inputs(
        &mut inputs,
        store,
        &skill.requires,
        ArtifactRelationship::Requires,
    );
    collect_inputs(
        &mut inputs,
        store,
        &skill.accepts,
        ArtifactRelationship::Accepts,
    );

    ContextInjection {
        skill: skill.name.clone(),
        inputs,
        expected_outputs: ExpectedOutputs {
            produces: skill.produces.clone(),
            may_produce: skill.may_produce.clone(),
        },
    }
}

fn collect_inputs(
    target: &mut Vec<ArtifactRef>,
    store: &ArtifactStore,
    artifact_types: &[String],
    relationship: ArtifactRelationship,
) {
    for artifact_type in artifact_types {
        for (instance_id, state) in store.instances_of(artifact_type) {
            if matches!(state.status, ValidationStatus::Valid) {
                target.push(ArtifactRef {
                    artifact_type: artifact_type.clone(),
                    instance_id: instance_id.to_string(),
                    path: state.path.clone(),
                    content_hash: state.content_hash.clone(),
                    relationship,
                });
            }
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

        let skill = SkillDeclaration {
            name: "implement".into(),
            requires: vec!["constraints".into()],
            accepts: vec!["notes".into()],
            produces: vec!["implementation".into()],
            may_produce: vec!["scratchpad".into()],
            trigger: crate::TriggerCondition::OnArtifact {
                name: "constraints".into(),
            },
        };

        let context = build_context(&skill, &store);
        assert_eq!(context.skill, "implement");
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
                path: constraints_path,
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
                path: notes_path,
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

        let skill = SkillDeclaration {
            name: "implement".into(),
            requires: vec!["constraints".into()],
            accepts: vec!["notes".into(), "missing".into()],
            produces: Vec::new(),
            may_produce: Vec::new(),
            trigger: crate::TriggerCondition::OnArtifact {
                name: "constraints".into(),
            },
        };

        let context = build_context(&skill, &store);
        assert_eq!(context.inputs.len(), 1);
        assert_eq!(context.inputs[0].artifact_type, "constraints");
        assert_eq!(
            context.inputs[0].relationship,
            ArtifactRelationship::Requires
        );
    }
}
