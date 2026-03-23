/// Re-export the core message types from kodo-llm.
///
/// This module exists so downstream crates can import from kodo-core
/// without depending on kodo-llm directly.
pub use kodo_llm::types::{
    CompletionRequest, CompletionResponse, ContentBlock, Message, ModelInfo, Role, StopReason,
    StreamEvent, ToolCallingSupport, ToolDefinition, Usage,
};
