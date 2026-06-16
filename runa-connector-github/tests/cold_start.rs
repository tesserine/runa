use runa_connector_github::GitHubConnector;
use runa_forge_capability::{ForgeConnector, ForgeOperation, validate_value};
use serde_json::json;

#[test]
fn github_cold_start_reads_configured_ticket_when_deployment_is_available() {
    let Some(repository) = env_optional("RUNA_LIVE_GITHUB_REPOSITORY") else {
        return;
    };
    let Some(ticket) = env_optional("RUNA_LIVE_GITHUB_TICKET") else {
        return;
    };

    let mut table = toml::Table::new();
    table.insert(
        "provider".to_string(),
        toml::Value::String("github".to_string()),
    );
    table.insert("repository".to_string(), toml::Value::String(repository));
    if let Some(token_env) = env_optional("RUNA_LIVE_GITHUB_TOKEN_ENV") {
        table.insert(
            "credentials".to_string(),
            toml::Value::Table(toml::Table::from_iter([(
                "env".to_string(),
                toml::Value::String(token_env),
            )])),
        );
    }
    let connector = GitHubConnector::from_value(&toml::Value::Table(table)).unwrap();
    let output = connector
        .call(ForgeOperation::ReadTicket, json!({ "reference": ticket }))
        .unwrap();
    let schema = connector
        .tool_set()
        .tools
        .into_iter()
        .find(|tool| tool.operation == ForgeOperation::ReadTicket)
        .unwrap()
        .output_schema;
    validate_value(&schema, &output).unwrap();
}

fn env_optional(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}
