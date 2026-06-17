use rmcp::model::Tool;
use runa_toolset::{ToolSet, compose_tool_sets};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

fn tool(name: &str) -> Tool {
    Tool::new(
        name.to_string(),
        format!("{name} tool"),
        Arc::new(serde_json::Map::from_iter([
            ("type".to_string(), json!("object")),
            ("additionalProperties".to_string(), json!(false)),
        ])),
    )
}

#[test]
fn composes_disjoint_tool_sets_into_one_surface() {
    let registry = compose_tool_sets(
        vec![
            ToolSet::new("driver", vec![tool("advance")]),
            ToolSet::new("forge", vec![tool("read-ticket")]),
        ],
        &HashMap::new(),
    )
    .expect("disjoint tool sets should compose");

    let names: Vec<_> = registry
        .tools()
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect();
    assert_eq!(names, ["advance", "read-ticket"]);
    assert_eq!(registry.resolve("read-ticket").unwrap().role, "forge");
}

#[test]
fn collisions_are_loud_unless_role_qualified_alias_exists() {
    let error = compose_tool_sets(
        vec![
            ToolSet::new("artifact", vec![tool("read-ticket")]),
            ToolSet::new("forge", vec![tool("read-ticket")]),
        ],
        &HashMap::new(),
    )
    .unwrap_err();

    let message = error.to_string();
    assert!(message.contains("artifact/read-ticket"), "{message}");
    assert!(message.contains("forge/read-ticket"), "{message}");

    let registry = compose_tool_sets(
        vec![
            ToolSet::new("artifact", vec![tool("read-ticket")]),
            ToolSet::new("forge", vec![tool("read-ticket")]),
        ],
        &HashMap::from([(
            "forge/read-ticket".to_string(),
            "forge-read-ticket".to_string(),
        )]),
    )
    .expect("explicit alias should resolve the collision");

    let names: Vec<_> = registry
        .tools()
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect();
    assert_eq!(names, ["read-ticket", "forge-read-ticket"]);
    assert_eq!(registry.resolve("forge-read-ticket").unwrap().role, "forge");
}
