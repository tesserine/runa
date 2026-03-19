use std::path::Path;

use rmcp::model::{GetPromptResult, PromptMessage, PromptMessageRole};
use serde_json::Value;

use libagent::ArtifactStore;
use libagent::context::{ArtifactRelationship, ContextInjection};

/// Render the agent-facing context prompt as natural language prose.
///
/// Reads artifact files from the paths in `ContextInjection`, parses as JSON,
/// and transforms to readable prose. Errors reading or parsing individual
/// artifacts are rendered inline rather than failing the prompt.
pub fn render_context_prompt(
    context: &ContextInjection,
    _store: &ArtifactStore,
    _workspace_dir: &Path,
) -> GetPromptResult {
    let mut sections = Vec::new();
    sections.push(format!("# Protocol: {}", context.protocol));

    // Required Inputs
    let required: Vec<_> = context
        .inputs
        .iter()
        .filter(|i| i.relationship == ArtifactRelationship::Requires)
        .collect();
    if !required.is_empty() {
        sections.push("\n## Required Inputs".to_string());
        for input in required {
            sections.push(render_input(input));
        }
    }

    // Available Inputs (accepts)
    let available: Vec<_> = context
        .inputs
        .iter()
        .filter(|i| i.relationship == ArtifactRelationship::Accepts)
        .collect();
    if !available.is_empty() {
        sections.push("\n## Available Inputs".to_string());
        for input in available {
            sections.push(render_input(input));
        }
    }

    // Expected Outputs
    sections.push("\n## Expected Outputs".to_string());
    if !context.expected_outputs.produces.is_empty() {
        let list = context.expected_outputs.produces.join(", ");
        sections.push(format!("\nYou must produce: {list}"));
    }
    if !context.expected_outputs.may_produce.is_empty() {
        let list = context.expected_outputs.may_produce.join(", ");
        sections.push(format!("You may also produce: {list}"));
    }

    let text = sections.join("\n");

    GetPromptResult {
        description: Some(format!("Context for protocol '{}'", context.protocol)),
        messages: vec![PromptMessage::new_text(PromptMessageRole::User, text)],
    }
}

fn render_input(input: &libagent::context::ArtifactRef) -> String {
    let mut result = format!("\n### {} — {}", input.artifact_type, input.instance_id);

    match std::fs::read_to_string(&input.path) {
        Ok(content) => match serde_json::from_str::<Value>(&content) {
            Ok(value) => {
                result.push('\n');
                result.push_str(&json_to_prose(&value, 0));
            }
            Err(e) => {
                result.push_str(&format!("\n(Could not parse artifact: {e})"));
            }
        },
        Err(e) => {
            result.push_str(&format!("\n(Could not read artifact: {e})"));
        }
    }

    result
}

/// Convert a JSON value to human-readable prose.
///
/// Objects become labeled key-value sections with humanized keys.
/// Arrays become numbered lists. Nested structures are indented.
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
                    _ => {
                        lines.push(format!("{prefix}**{label}:** {}", inline_value(val)));
                    }
                }
            }
            lines.join("\n")
        }
        Value::Array(arr) => {
            let mut lines = Vec::new();
            for (i, item) in arr.iter().enumerate() {
                let num = i + 1;
                match item {
                    Value::Object(_) | Value::Array(_) => {
                        lines.push(format!("{prefix}{num}."));
                        lines.push(json_to_prose(item, indent + 1));
                    }
                    _ => {
                        lines.push(format!("{prefix}{num}. {}", inline_value(item)));
                    }
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

/// Convert a snake_case key to a human-readable label.
///
/// `"work_unit"` → `"Work unit"`, `"acceptance_criteria"` → `"Acceptance criteria"`.
fn humanize_key(key: &str) -> String {
    let mut result = key.replace('_', " ");
    if let Some(first) = result.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn humanize_key_snake_case() {
        assert_eq!(humanize_key("work_unit"), "Work unit");
        assert_eq!(humanize_key("acceptance_criteria"), "Acceptance criteria");
        assert_eq!(humanize_key("title"), "Title");
    }

    #[test]
    fn inline_value_formats_primitives() {
        assert_eq!(inline_value(&json!("hello")), "hello");
        assert_eq!(inline_value(&json!(42)), "42");
        assert_eq!(inline_value(&json!(true)), "true");
        assert_eq!(inline_value(&json!(null)), "null");
    }

    #[test]
    fn json_to_prose_flat_object() {
        let val = json!({"title": "Schema validation", "work_unit": "feature-x"});
        let prose = json_to_prose(&val, 0);
        assert!(prose.contains("**Title:** Schema validation"));
        assert!(prose.contains("**Work unit:** feature-x"));
    }

    #[test]
    fn json_to_prose_nested_object() {
        let val = json!({
            "title": "Test",
            "details": {
                "author": "agent",
                "priority": "high"
            }
        });
        let prose = json_to_prose(&val, 0);
        assert!(prose.contains("**Details:**"));
        assert!(prose.contains("  **Author:** agent"));
        assert!(prose.contains("  **Priority:** high"));
    }

    #[test]
    fn json_to_prose_array() {
        let val = json!({"scenarios": ["first", "second", "third"]});
        let prose = json_to_prose(&val, 0);
        assert!(prose.contains("**Scenarios:**"));
        assert!(prose.contains("  1. first"));
        assert!(prose.contains("  2. second"));
        assert!(prose.contains("  3. third"));
    }

    #[test]
    fn json_to_prose_nested_array_of_objects() {
        let val = json!({
            "scenarios": [
                {"name": "reject-missing-field", "criterion": "returns 400"}
            ]
        });
        let prose = json_to_prose(&val, 0);
        assert!(prose.contains("**Scenarios:**"));
        assert!(prose.contains("1."));
        assert!(prose.contains("**Name:** reject-missing-field"));
        assert!(prose.contains("**Criterion:** returns 400"));
    }
}
