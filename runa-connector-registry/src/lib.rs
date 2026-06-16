use std::collections::HashMap;
use std::sync::Arc;

use runa_connector_github::GitHubConnector;
use runa_connector_sourcehut::SourceHutConnector;
use runa_forge_capability::{ForgeConnector, ForgeError, ForgeOperation, ForgeTool};
use serde_json::{Map, Value};

#[derive(Clone)]
pub struct ConnectorCatalog {
    connectors: Vec<Arc<dyn ForgeConnector>>,
    aliases: HashMap<String, String>,
}

#[derive(Clone)]
pub struct ConnectorToolBinding {
    pub exposed_name: String,
    pub set_label: String,
    pub operation: ForgeOperation,
    pub connector: Arc<dyn ForgeConnector>,
    pub tool: ForgeTool,
}

impl ConnectorCatalog {
    pub fn empty() -> Self {
        Self {
            connectors: Vec::new(),
            aliases: HashMap::new(),
        }
    }

    pub fn new(connectors: Vec<Arc<dyn ForgeConnector>>, aliases: HashMap<String, String>) -> Self {
        Self {
            connectors,
            aliases,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.connectors.is_empty()
    }

    pub fn compose(
        &self,
        reserved_tools: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Vec<ConnectorToolBinding>, ForgeError> {
        let mut occupied: HashMap<String, String> = reserved_tools.into_iter().collect();
        let mut bindings = Vec::new();

        for connector in &self.connectors {
            let tool_set = connector.tool_set();
            for tool in tool_set.tools {
                let role_key = format!("forge:{}", tool.name);
                let exposed_name = self
                    .aliases
                    .get(&role_key)
                    .cloned()
                    .unwrap_or_else(|| tool.name.clone());
                if let Some(existing_label) = occupied.get(&exposed_name) {
                    return Err(ForgeError::new(format!(
                        "tool name collision for '{exposed_name}' between '{existing_label}' and '{}'; declare alias '{} = \"...\"'",
                        tool_set.label, role_key
                    )));
                }
                occupied.insert(exposed_name.clone(), tool_set.label.clone());
                bindings.push(ConnectorToolBinding {
                    exposed_name,
                    set_label: tool_set.label.clone(),
                    operation: tool.operation,
                    connector: Arc::clone(connector),
                    tool,
                });
            }
        }

        Ok(bindings)
    }
}

pub fn load_from_config(connectors: &toml::Table) -> Result<ConnectorCatalog, ForgeError> {
    let aliases = parse_aliases(connectors.get("aliases"))?;
    let Some(forge) = connectors.get("forge") else {
        return Ok(ConnectorCatalog::empty());
    };
    let provider = forge
        .get("provider")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| ForgeError::new("connectors.forge.provider is required"))?;
    let connector: Arc<dyn ForgeConnector> = match provider {
        "github" => Arc::new(GitHubConnector::from_value(forge)?),
        "sourcehut" => Arc::new(SourceHutConnector::from_value(forge)?),
        other => {
            return Err(ForgeError::new(format!(
                "unknown forge connector provider: {other}"
            )));
        }
    };
    Ok(ConnectorCatalog::new(vec![connector], aliases))
}

pub fn schema_object_to_map(schema: &Map<String, Value>) -> Map<String, Value> {
    schema.clone()
}

fn parse_aliases(value: Option<&toml::Value>) -> Result<HashMap<String, String>, ForgeError> {
    let Some(value) = value else {
        return Ok(HashMap::new());
    };
    let table = value
        .as_table()
        .ok_or_else(|| ForgeError::new("connectors.aliases must be a table"))?;
    let mut aliases = HashMap::new();
    for (role_key, alias_value) in table {
        let alias = alias_value.as_str().ok_or_else(|| {
            ForgeError::new(format!("connectors.aliases.{role_key} must be a string"))
        })?;
        aliases.insert(role_key.clone(), alias.to_string());
    }
    Ok(aliases)
}

#[cfg(test)]
mod tests {
    use super::*;
    use runa_forge_capability::{ForgeToolSet, canonical_tool_set};
    use serde_json::json;

    struct StaticConnector;

    impl ForgeConnector for StaticConnector {
        fn provider(&self) -> &'static str {
            "static"
        }

        fn tool_set(&self) -> ForgeToolSet {
            canonical_tool_set("forge:static")
        }

        fn call(&self, _operation: ForgeOperation, _input: Value) -> Result<Value, ForgeError> {
            Ok(json!({}))
        }
    }

    #[test]
    fn collision_is_loud_without_alias() {
        let catalog = ConnectorCatalog::new(vec![Arc::new(StaticConnector)], HashMap::new());
        let err = match catalog.compose([(
            "read-ticket".to_string(),
            "artifact-output:take".to_string(),
        )]) {
            Ok(_) => panic!("collision should fail without alias"),
            Err(error) => error,
        };
        let message = err.to_string();
        assert!(message.contains("read-ticket"));
        assert!(message.contains("artifact-output:take"));
        assert!(message.contains("forge:static"));
        assert!(message.contains("forge:read-ticket"));
    }

    #[test]
    fn alias_allows_collision_to_compose() {
        let catalog = ConnectorCatalog::new(
            vec![Arc::new(StaticConnector)],
            HashMap::from([(
                "forge:read-ticket".to_string(),
                "forge-read-ticket".to_string(),
            )]),
        );
        let bindings = catalog
            .compose([(
                "read-ticket".to_string(),
                "artifact-output:take".to_string(),
            )])
            .unwrap();
        assert!(
            bindings
                .iter()
                .any(|binding| binding.exposed_name == "forge-read-ticket")
        );
    }
}
