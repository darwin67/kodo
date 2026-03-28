use std::pin::Pin;

use anyhow::{Context as _, Result};
use async_trait::async_trait;
use futures::Stream;
use reqwest::Client;
use serde::Deserialize;

use crate::openai::OpenAiProvider;
use crate::provider::Provider;
use crate::types::{
    CompletionRequest, CompletionResponse, ModelInfo, StreamEvent, ToolCallingSupport,
};

const DEFAULT_OLLAMA_BASE: &str = "http://localhost:11434/v1";

/// Ollama provider — wraps the OpenAI-compatible API that Ollama exposes.
///
/// Ollama runs locally and serves an OpenAI-compatible endpoint at
/// `http://localhost:11434/v1`. No API key is needed by default.
pub struct OllamaProvider {
    inner: OpenAiProvider,
    base_url: String,
}

impl OllamaProvider {
    /// Create an Ollama provider connecting to the default local endpoint.
    pub fn new() -> Self {
        Self::with_base_url(DEFAULT_OLLAMA_BASE.to_string())
    }

    /// Create with a custom base URL.
    pub fn with_base_url(base_url: String) -> Self {
        let inner = OpenAiProvider::new("ollama".into()).with_base_url(base_url.clone());
        Self { inner, base_url }
    }

    /// Create from the `OLLAMA_HOST` environment variable, or fall back to localhost.
    pub fn from_env() -> Self {
        let host = std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://localhost:11434".into());
        let base_url = format!("{host}/v1");
        Self::with_base_url(base_url)
    }

    /// Check if Ollama is reachable by hitting the /api/tags endpoint.
    pub async fn is_available(&self) -> bool {
        let tags_url = self.base_url.replace("/v1", "/api/tags");
        let client = Client::new();
        client
            .get(&tags_url)
            .timeout(std::time::Duration::from_secs(2))
            .send()
            .await
            .is_ok_and(|r| r.status().is_success())
    }

    /// Fetch the list of locally available models from Ollama.
    pub async fn list_local_models(&self) -> Result<Vec<ModelInfo>> {
        let tags_url = self.base_url.replace("/v1", "/api/tags");
        let client = Client::new();

        let resp = client
            .get(&tags_url)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .context("failed to reach Ollama")?;

        let body: OllamaTagsResponse = resp
            .json()
            .await
            .context("failed to parse Ollama tags response")?;

        Ok(body
            .models
            .into_iter()
            .map(|m| ModelInfo {
                id: m.name.clone(),
                name: m.name,
                context_window: 0, // Ollama doesn't expose this in /api/tags
            })
            .collect())
    }
}

#[derive(Deserialize)]
struct OllamaTagsResponse {
    models: Vec<OllamaModel>,
}

#[derive(Deserialize)]
struct OllamaModel {
    name: String,
}

impl Default for OllamaProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for OllamaProvider {
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse> {
        self.inner.complete(request).await
    }

    async fn stream(
        &self,
        request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        self.inner.stream(request).await
    }

    fn tool_calling_support(&self) -> ToolCallingSupport {
        // Most Ollama models support tool calling through the OpenAI-compat API,
        // but smaller models may not. Default to Native since the API supports it.
        ToolCallingSupport::Native
    }

    fn name(&self) -> &str {
        "ollama"
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        self.list_local_models().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_base_url() {
        let provider = OllamaProvider::new();
        assert_eq!(provider.base_url, DEFAULT_OLLAMA_BASE);
    }

    #[test]
    fn custom_base_url() {
        let provider = OllamaProvider::with_base_url("http://remote:11434/v1".into());
        assert_eq!(provider.base_url, "http://remote:11434/v1");
    }

    #[test]
    fn provider_name() {
        let provider = OllamaProvider::new();
        assert_eq!(provider.name(), "ollama");
    }

    #[test]
    fn provider_tool_calling_support() {
        let provider = OllamaProvider::new();
        assert_eq!(provider.tool_calling_support(), ToolCallingSupport::Native);
    }

    #[test]
    fn from_env_uses_default_when_no_env() {
        let original = std::env::var("OLLAMA_HOST").ok();
        unsafe { std::env::remove_var("OLLAMA_HOST") };

        let provider = OllamaProvider::from_env();
        assert!(provider.base_url.contains("localhost:11434"));

        if let Some(host) = original {
            unsafe { std::env::set_var("OLLAMA_HOST", host) };
        }
    }

    #[tokio::test]
    async fn is_available_returns_false_on_unreachable() {
        let provider = OllamaProvider::with_base_url("http://127.0.0.1:1/v1".into());
        assert!(!provider.is_available().await);
    }
}
