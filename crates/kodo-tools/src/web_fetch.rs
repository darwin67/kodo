use std::time::Duration;

use anyhow::Result;
use tracing::debug;

use crate::tool::{PermissionLevel, Tool, ToolContext, ToolOutput};

const MAX_BODY_BYTES: usize = 102_400; // 100 KB
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Tool that fetches content from a URL.
pub struct WebFetchTool;

impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        "Fetch content from a URL and return the response body as text. \
         Useful for reading documentation, API responses, or web content."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Timeout in seconds. Default 30."
                }
            },
            "required": ["url"]
        })
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
    }

    fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &ToolContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ToolOutput>> + Send + '_>> {
        Box::pin(async move {
            let url = params
                .get("url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("missing required parameter: url"))?;

            let timeout_secs = params
                .get("timeout_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(DEFAULT_TIMEOUT_SECS);

            debug!(url, timeout_secs, "fetching URL");

            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(timeout_secs))
                .build()?;

            let response = client.get(url).send().await?;

            let status = response.status();
            let headers = response.headers().clone();
            let content_type = headers
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("unknown")
                .to_string();

            let body = response.text().await?;

            let truncated = body.len() > MAX_BODY_BYTES;
            let body_display = if truncated {
                format!(
                    "{}\n\n... (response truncated at {} bytes, total {} bytes)",
                    &body[..MAX_BODY_BYTES],
                    MAX_BODY_BYTES,
                    body.len()
                )
            } else {
                body
            };

            let mut output = format!("HTTP {} | Content-Type: {}\n\n", status, content_type);
            output.push_str(&body_display);

            Ok(ToolOutput {
                content: output,
                success: status.is_success(),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ctx() -> ToolContext {
        ToolContext {
            working_dir: std::env::current_dir().unwrap(),
        }
    }

    #[tokio::test]
    async fn web_fetch_missing_url() {
        let tool = WebFetchTool;
        let ctx = make_ctx();
        let params = serde_json::json!({});
        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn web_fetch_invalid_url() {
        let tool = WebFetchTool;
        let ctx = make_ctx();
        let params = serde_json::json!({"url": "not-a-url"});
        let result = tool.execute(params, &ctx).await;
        // reqwest should fail on invalid URLs
        assert!(result.is_err());
    }

    // Note: We don't test actual HTTP requests in unit tests to avoid
    // network dependency. Integration tests can cover that.
}
