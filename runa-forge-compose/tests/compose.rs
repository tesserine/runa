use libagent::ForgeConfig;
use runa_forge_compose::runtime_from_config;
use std::collections::BTreeMap;

#[test]
fn github_config_loads_a_forge_runtime_with_output_schemas() {
    let runtime = runtime_from_config(&ForgeConfig {
        forge_type: Some("github".to_string()),
        owner: Some("tesserine".to_string()),
        name: Some("runa".to_string()),
        api_base: Some("https://api.github.test".to_string()),
        ..ForgeConfig::default()
    })
    .unwrap()
    .expect("github config should load a connector");

    let read_ticket = runtime.tools.get("read-ticket").unwrap();
    assert_eq!(runtime.tools.len(), 8);
    assert_eq!(read_ticket.operation.canonical_name(), "read-ticket");
    assert!(read_ticket.output_schema.get("$defs").is_some());
}

#[test]
fn sourcehut_config_loads_a_forge_runtime() {
    let runtime = runtime_from_config(&ForgeConfig {
        forge_type: Some("sourcehut".to_string()),
        tracker_id: Some("4".to_string()),
        api_base: Some("https://todo.example/query".to_string()),
        git_remote: Some("ssh://git@git.example/runa".to_string()),
        ..ForgeConfig::default()
    })
    .unwrap()
    .expect("sourcehut config should load a connector");

    assert_eq!(runtime.tools.len(), 8);
    assert!(runtime.tools.contains_key("create-ticket"));
}

#[test]
fn explicit_aliases_are_applied_at_composition() {
    let mut aliases = BTreeMap::new();
    aliases.insert(
        "github:read-ticket".to_string(),
        "work-unit-read-ticket".to_string(),
    );
    let runtime = runtime_from_config(&ForgeConfig {
        forge_type: Some("github".to_string()),
        owner: Some("tesserine".to_string()),
        name: Some("runa".to_string()),
        tool_aliases: aliases,
        ..ForgeConfig::default()
    })
    .unwrap()
    .expect("github config should load a connector");

    assert!(!runtime.tools.contains_key("read-ticket"));
    assert!(runtime.tools.contains_key("work-unit-read-ticket"));
}

#[test]
fn absent_connector_config_leaves_the_mcp_surface_unchanged() {
    assert!(
        runtime_from_config(&ForgeConfig::default())
            .unwrap()
            .is_none()
    );
}
