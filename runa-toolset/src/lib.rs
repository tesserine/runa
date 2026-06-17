//! Generic MCP tool-set composition for runa.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;

use rmcp::model::Tool;

#[derive(Debug, Clone)]
pub struct ToolSet {
    pub role: String,
    pub tools: Vec<Tool>,
}

impl ToolSet {
    pub fn new(role: impl Into<String>, tools: Vec<Tool>) -> Self {
        Self {
            role: role.into(),
            tools,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSource {
    pub role: String,
    pub local_name: String,
}

#[derive(Debug, Clone)]
pub struct ToolRegistry {
    tools: Vec<Tool>,
    sources: HashMap<String, ToolSource>,
}

impl ToolRegistry {
    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }

    pub fn resolve(&self, exposed_name: &str) -> Option<&ToolSource> {
        self.sources.get(exposed_name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposeError {
    Collision {
        exposed_name: String,
        first: ToolSource,
        second: ToolSource,
    },
    AliasTargetCollision {
        exposed_name: String,
        first: ToolSource,
        second: ToolSource,
    },
}

impl fmt::Display for ComposeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ComposeError::Collision {
                exposed_name,
                first,
                second,
            } => write!(
                f,
                "tool name collision on '{exposed_name}' between {}/{} and {}/{}",
                first.role, first.local_name, second.role, second.local_name
            ),
            ComposeError::AliasTargetCollision {
                exposed_name,
                first,
                second,
            } => write!(
                f,
                "tool alias target '{exposed_name}' collides between {}/{} and {}/{}",
                first.role, first.local_name, second.role, second.local_name
            ),
        }
    }
}

impl std::error::Error for ComposeError {}

pub fn compose_tool_sets(
    tool_sets: Vec<ToolSet>,
    aliases: &HashMap<String, String>,
) -> Result<ToolRegistry, ComposeError> {
    let mut tools = Vec::new();
    let mut sources = HashMap::new();

    for tool_set in tool_sets {
        for mut tool in tool_set.tools {
            let local_name = tool.name.to_string();
            let source = ToolSource {
                role: tool_set.role.clone(),
                local_name: local_name.clone(),
            };
            let qualified = format!("{}/{}", source.role, source.local_name);
            let exposed_name = aliases.get(&qualified).cloned().unwrap_or(local_name);
            tool.name = Cow::Owned(exposed_name.clone());

            if let Some(first) = sources.insert(exposed_name.clone(), source.clone()) {
                let error = if aliases.contains_key(&qualified) {
                    ComposeError::AliasTargetCollision {
                        exposed_name,
                        first,
                        second: source,
                    }
                } else {
                    ComposeError::Collision {
                        exposed_name,
                        first,
                        second: source,
                    }
                };
                return Err(error);
            }
            tools.push(tool);
        }
    }

    Ok(ToolRegistry { tools, sources })
}
