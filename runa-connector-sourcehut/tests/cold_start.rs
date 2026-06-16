use runa_connector_sourcehut::SourceHutConnector;
use runa_forge_capability::{ForgeConnector, ForgeOperation, validate_value};
use serde_json::json;

#[test]
fn sourcehut_cold_start_uses_configured_endpoint_and_rejects_default_host() {
    let Some(endpoint) = env_optional("RUNA_LIVE_SOURCEHUT_ENDPOINT") else {
        return;
    };
    let Some(owner) = env_optional("RUNA_LIVE_SOURCEHUT_OWNER") else {
        return;
    };
    let Some(name) = env_optional("RUNA_LIVE_SOURCEHUT_NAME") else {
        return;
    };
    let Some(tracker_id) =
        env_optional("RUNA_LIVE_SOURCEHUT_TRACKER_ID").and_then(|value| value.parse::<i64>().ok())
    else {
        return;
    };
    let Some(ticket) = env_optional("RUNA_LIVE_SOURCEHUT_TICKET") else {
        return;
    };
    let Some(token_env) = env_optional("RUNA_LIVE_SOURCEHUT_TOKEN_ENV") else {
        return;
    };

    let configured_connector = connector(&endpoint, &owner, &name, tracker_id, &token_env);
    let output = configured_connector
        .call(ForgeOperation::ReadTicket, json!({ "reference": ticket }))
        .unwrap();
    let schema = configured_connector
        .tool_set()
        .tools
        .into_iter()
        .find(|tool| tool.operation == ForgeOperation::ReadTicket)
        .unwrap()
        .output_schema;
    validate_value(&schema, &output).unwrap();

    if endpoint != "sr.ht" {
        let default_host_connector = connector("sr.ht", &owner, &name, tracker_id, &token_env);
        let wrong_endpoint_result =
            default_host_connector.call(ForgeOperation::ReadTicket, json!({ "reference": ticket }));
        assert!(
            wrong_endpoint_result.is_err(),
            "default SourceHut host config must fail for non-default deployment endpoint"
        );
    }
}

fn connector(
    endpoint: &str,
    owner: &str,
    name: &str,
    tracker_id: i64,
    token_env: &str,
) -> SourceHutConnector {
    SourceHutConnector::from_value(&toml::Value::Table(toml::Table::from_iter([
        (
            "provider".to_string(),
            toml::Value::String("sourcehut".to_string()),
        ),
        (
            "endpoint".to_string(),
            toml::Value::String(endpoint.to_string()),
        ),
        ("owner".to_string(), toml::Value::String(owner.to_string())),
        ("name".to_string(), toml::Value::String(name.to_string())),
        ("tracker_id".to_string(), toml::Value::Integer(tracker_id)),
        (
            "credentials".to_string(),
            toml::Value::Table(toml::Table::from_iter([(
                "env".to_string(),
                toml::Value::String(token_env.to_string()),
            )])),
        ),
    ])))
    .unwrap()
}

fn env_optional(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}
