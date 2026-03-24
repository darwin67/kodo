pub mod file_edit;
pub mod file_read;
pub mod file_write;
pub mod glob_search;
pub mod grep_search;
pub mod registry;
pub mod shell;
pub mod tool;
pub mod web_fetch;

use std::sync::Arc;

use registry::ToolRegistry;

/// Register all built-in tools into the given registry.
pub fn register_builtin_tools(registry: &mut ToolRegistry) {
    registry.register(Arc::new(file_read::FileReadTool));
    registry.register(Arc::new(file_write::FileWriteTool));
    registry.register(Arc::new(file_edit::FileEditTool));
    registry.register(Arc::new(shell::ShellTool));
    registry.register(Arc::new(glob_search::GlobSearchTool));
    registry.register(Arc::new(grep_search::GrepSearchTool));
    registry.register(Arc::new(web_fetch::WebFetchTool));
}
