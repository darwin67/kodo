use std::pin::Pin;

use anyhow::Result;
use async_trait::async_trait;
use futures::Stream;

use crate::types::{
    CompletionRequest, CompletionResponse, ModelInfo, StreamEvent, ToolCallingSupport,
};

/// A language model provider that can generate completions.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Send a conversation and get a complete response.
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse>;

    /// Stream a response event by event.
    async fn stream(
        &self,
        request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>>;

    /// Whether this provider supports native tool calling.
    fn tool_calling_support(&self) -> ToolCallingSupport;

    /// Provider display name.
    fn name(&self) -> &str;

    /// List available models for this provider.
    async fn list_models(&self) -> Result<Vec<ModelInfo>>;
}
