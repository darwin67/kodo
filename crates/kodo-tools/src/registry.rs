use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, bail};
use tracing::debug;

use crate::tool::{Tool, ToolContext, ToolOutput};

/// Registry of available tools, keyed by name.
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool. operation is idempotent
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        let name = tool.name().to_string();
        if self.tools.contains_key(&name) {
            debug!("tool already registered: {}", name);
            return;
        }
        self.tools.insert(name, tool);
    }

    /// Get a tool by name.
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(name)
    }

    /// Execute a tool by name with the given parameters.
    pub async fn execute(
        &self,
        name: &str,
        params: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let tool = match self.get(name) {
            Some(t) => t,
            None => bail!("unknown tool: {name}"),
        };
        tool.execute(params, ctx).await
    }

    /// Return tool definitions formatted for LLM consumption.
    pub fn tool_definitions(&self) -> Vec<serde_json::Value> {
        self.tools
            .values()
            .map(|tool| {
                serde_json::json!({
                    "name": tool.name(),
                    "description": tool.description(),
                    "input_schema": tool.parameters_schema(),
                })
            })
            .collect()
    }

    /// List all registered tool names.
    pub fn names(&self) -> Vec<&str> {
        self.tools.keys().map(|s| s.as_str()).collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{PermissionLevel, ToolOutput};
    use std::path::PathBuf;

    /// A dummy tool for testing the registry.
    struct DummyTool {
        tool_name: &'static str,
    }

    impl DummyTool {
        fn new(name: &'static str) -> Self {
            Self { tool_name: name }
        }
    }

    impl Tool for DummyTool {
        fn name(&self) -> &str {
            self.tool_name
        }

        fn description(&self) -> &str {
            "A dummy tool for testing"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {}
            })
        }

        fn permission_level(&self) -> PermissionLevel {
            PermissionLevel::Read
        }

        fn execute(
            &self,
            _params: serde_json::Value,
            _ctx: &ToolContext,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = anyhow::Result<ToolOutput>> + Send + '_>,
        > {
            Box::pin(async {
                Ok(ToolOutput {
                    content: format!("executed {}", self.tool_name),
                    success: true,
                })
            })
        }
    }

    fn make_ctx() -> ToolContext {
        ToolContext {
            working_dir: PathBuf::from("/tmp"),
        }
    }

    #[test]
    fn new_registry_is_empty() {
        let registry = ToolRegistry::new();
        assert!(registry.names().is_empty());
        assert!(registry.tool_definitions().is_empty());
    }

    #[test]
    fn default_registry_is_empty() {
        let registry = ToolRegistry::default();
        assert!(registry.names().is_empty());
    }

    #[test]
    fn register_and_get_tool() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(DummyTool::new("test_tool")));

        assert!(registry.get("test_tool").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn register_multiple_tools() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(DummyTool::new("tool_a")));
        registry.register(Arc::new(DummyTool::new("tool_b")));
        registry.register(Arc::new(DummyTool::new("tool_c")));

        let mut names = registry.names();
        names.sort();
        assert_eq!(names, vec!["tool_a", "tool_b", "tool_c"]);
    }

    #[test]
    fn register_duplicate_tool_is_idempotent() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(DummyTool::new("duplicate")));
        registry.register(Arc::new(DummyTool::new("duplicate")));

        // Should have only one tool registered
        assert_eq!(registry.names().len(), 1);
        assert!(registry.get("duplicate").is_some());
    }

    #[test]
    fn tool_definitions_returns_correct_format() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(DummyTool::new("my_tool")));

        let defs = registry.tool_definitions();
        assert_eq!(defs.len(), 1);

        let def = &defs[0];
        assert_eq!(def["name"], "my_tool");
        assert_eq!(def["description"], "A dummy tool for testing");
        assert!(def["input_schema"].is_object());
    }

    #[tokio::test]
    async fn execute_known_tool() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(DummyTool::new("my_tool")));

        let ctx = make_ctx();
        let result = registry
            .execute("my_tool", serde_json::json!({}), &ctx)
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.content, "executed my_tool");
    }

    #[tokio::test]
    async fn execute_unknown_tool_returns_error() {
        let registry = ToolRegistry::new();
        let ctx = make_ctx();
        let result = registry
            .execute("nonexistent", serde_json::json!({}), &ctx)
            .await;

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("unknown tool: nonexistent"));
    }
}
