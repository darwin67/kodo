use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Permission level required to execute a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionLevel {
    /// Read-only operations (file read, search, web fetch).
    Read,
    /// Write operations (file write, file edit).
    Write,
    /// Execute operations (shell commands).
    Execute,
}

/// The result of executing a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    /// The textual output to feed back to the LLM.
    pub content: String,
    /// Whether the tool execution was successful.
    pub success: bool,
}

/// Context provided to tool execution.
pub struct ToolContext {
    /// The working directory for the current session.
    pub working_dir: std::path::PathBuf,
}

/// A tool that the agent can invoke.
///
/// Uses a boxed future return type instead of `async fn` for dyn-compatibility.
pub trait Tool: Send + Sync {
    /// Unique name used to identify this tool in LLM tool calls.
    fn name(&self) -> &str;

    /// Human-readable description of what this tool does.
    fn description(&self) -> &str;

    /// JSON Schema describing the parameters this tool accepts.
    fn parameters_schema(&self) -> serde_json::Value;

    /// The permission level required to execute this tool.
    fn permission_level(&self) -> PermissionLevel;

    /// Execute the tool with the given parameters.
    fn execute(
        &self,
        params: serde_json::Value,
        ctx: &ToolContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ToolOutput>> + Send + '_>>;
}
