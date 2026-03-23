use std::pin::Pin;

use anyhow::{Context as _, Result, bail};
use async_trait::async_trait;
use futures::stream::{self, Stream, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::provider::Provider;
use crate::types::{
    CompletionRequest, CompletionResponse, ContentBlock, Message, ModelInfo, Role, StopReason,
    StreamEvent, ToolCallingSupport, ToolDefinition, Usage,
};

const API_BASE: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";

// ---------------------------------------------------------------------------
// Anthropic API request/response types (private)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ApiRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<ApiMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ApiTool>,
    stream: bool,
}

#[derive(Serialize)]
struct ApiMessage {
    role: String,
    content: Vec<ApiContentBlock>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ApiContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

#[derive(Serialize)]
struct ApiTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

#[derive(Deserialize, Debug)]
struct ApiResponse {
    content: Vec<ApiContentBlock>,
    stop_reason: Option<String>,
    usage: ApiUsage,
}

#[derive(Deserialize, Debug, Clone, Copy, Default)]
struct ApiUsage {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
}

#[derive(Deserialize, Debug)]
struct ApiError {
    error: ApiErrorDetail,
}

#[derive(Deserialize, Debug)]
struct ApiErrorDetail {
    message: String,
}

// ---------------------------------------------------------------------------
// SSE event types
// ---------------------------------------------------------------------------

#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SseData {
    MessageStart {
        message: SseMessageStart,
    },
    ContentBlockStart {
        #[allow(dead_code)]
        index: usize,
        content_block: ApiContentBlock,
    },
    ContentBlockDelta {
        #[allow(dead_code)]
        index: usize,
        delta: SseDelta,
    },
    ContentBlockStop {
        #[allow(dead_code)]
        index: usize,
    },
    MessageDelta {
        delta: SseMessageDeltaPayload,
        usage: ApiUsage,
    },
    MessageStop,
    Ping,
    Error {
        error: ApiErrorDetail,
    },
}

#[derive(Deserialize, Debug)]
struct SseMessageStart {
    usage: ApiUsage,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SseDelta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
}

#[derive(Deserialize, Debug)]
struct SseMessageDeltaPayload {
    stop_reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

fn to_api_messages(messages: &[Message]) -> Vec<ApiMessage> {
    messages
        .iter()
        .map(|m| ApiMessage {
            role: match m.role {
                Role::User => "user".into(),
                Role::Assistant => "assistant".into(),
            },
            content: m.content.iter().map(to_api_content_block).collect(),
        })
        .collect()
}

fn to_api_content_block(block: &ContentBlock) -> ApiContentBlock {
    match block {
        ContentBlock::Text { text } => ApiContentBlock::Text { text: text.clone() },
        ContentBlock::ToolUse { id, name, input } => ApiContentBlock::ToolUse {
            id: id.clone(),
            name: name.clone(),
            input: input.clone(),
        },
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => ApiContentBlock::ToolResult {
            tool_use_id: tool_use_id.clone(),
            content: content.clone(),
            is_error: *is_error,
        },
    }
}

fn to_api_tools(tools: &[ToolDefinition]) -> Vec<ApiTool> {
    tools
        .iter()
        .map(|t| ApiTool {
            name: t.name.clone(),
            description: t.description.clone(),
            input_schema: t.input_schema.clone(),
        })
        .collect()
}

fn from_api_content_block(block: &ApiContentBlock) -> ContentBlock {
    match block {
        ApiContentBlock::Text { text } => ContentBlock::text(text),
        ApiContentBlock::ToolUse { id, name, input } => {
            ContentBlock::tool_use(id, name, input.clone())
        }
        ApiContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => ContentBlock::ToolResult {
            tool_use_id: tool_use_id.clone(),
            content: content.clone(),
            is_error: *is_error,
        },
    }
}

fn parse_stop_reason(reason: Option<&str>) -> StopReason {
    match reason {
        Some("tool_use") => StopReason::ToolUse,
        Some("max_tokens") => StopReason::MaxTokens,
        _ => StopReason::EndTurn,
    }
}

// ---------------------------------------------------------------------------
// AnthropicProvider
// ---------------------------------------------------------------------------

/// Anthropic Claude API provider.
pub struct AnthropicProvider {
    client: Client,
    api_key: String,
}

impl AnthropicProvider {
    /// Create a new provider from an API key.
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
        }
    }

    /// Create from the `ANTHROPIC_API_KEY` environment variable.
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .context("ANTHROPIC_API_KEY environment variable not set")?;
        Ok(Self::new(api_key))
    }

    fn build_api_request(&self, request: &CompletionRequest, stream: bool) -> ApiRequest {
        ApiRequest {
            model: request.model.clone(),
            max_tokens: request.max_tokens,
            system: request.system.clone(),
            messages: to_api_messages(&request.messages),
            tools: to_api_tools(&request.tools),
            stream,
        }
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse> {
        let api_req = self.build_api_request(&request, false);

        let resp = self
            .client
            .post(format!("{API_BASE}/v1/messages"))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .json(&api_req)
            .send()
            .await
            .context("failed to send request to Anthropic API")?;

        let status = resp.status();
        let body = resp.text().await?;

        if !status.is_success() {
            let err: ApiError = serde_json::from_str(&body).unwrap_or(ApiError {
                error: ApiErrorDetail {
                    message: body.clone(),
                },
            });
            bail!("Anthropic API error ({}): {}", status, err.error.message);
        }

        let api_resp: ApiResponse =
            serde_json::from_str(&body).context("failed to parse Anthropic API response")?;

        let content: Vec<ContentBlock> = api_resp
            .content
            .iter()
            .map(from_api_content_block)
            .collect();

        Ok(CompletionResponse {
            message: Message {
                role: Role::Assistant,
                content,
            },
            stop_reason: parse_stop_reason(api_resp.stop_reason.as_deref()),
            usage: Usage {
                input_tokens: api_resp.usage.input_tokens,
                output_tokens: api_resp.usage.output_tokens,
            },
        })
    }

    async fn stream(
        &self,
        request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let api_req = self.build_api_request(&request, true);

        let resp = self
            .client
            .post(format!("{API_BASE}/v1/messages"))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .json(&api_req)
            .send()
            .await
            .context("failed to send streaming request to Anthropic API")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await?;
            let err: ApiError = serde_json::from_str(&body).unwrap_or(ApiError {
                error: ApiErrorDetail {
                    message: body.clone(),
                },
            });
            bail!("Anthropic API error ({}): {}", status, err.error.message);
        }

        let byte_stream = resp.bytes_stream();

        // Parse the SSE byte stream into StreamEvents.
        let event_stream = {
            let buffer = String::new();
            let current_event_type = String::new();

            stream::unfold(
                (byte_stream, buffer, current_event_type),
                |(mut byte_stream, mut buffer, mut current_event_type)| async move {
                    loop {
                        // Try to extract a complete line from the buffer.
                        if let Some(newline_pos) = buffer.find('\n') {
                            let line = buffer[..newline_pos].trim_end_matches('\r').to_string();
                            buffer = buffer[newline_pos + 1..].to_string();

                            if line.starts_with("event: ") {
                                current_event_type = line["event: ".len()..].to_string();
                                continue;
                            }

                            if line.starts_with("data: ") {
                                let data = &line["data: ".len()..];
                                debug!(event_type = %current_event_type, "SSE event");

                                match serde_json::from_str::<SseData>(data) {
                                    Ok(sse) => {
                                        let event = match sse {
                                            SseData::MessageStart { message } => {
                                                Some(Ok(StreamEvent::MessageStart {
                                                    usage: Usage {
                                                        input_tokens: message.usage.input_tokens,
                                                        output_tokens: message.usage.output_tokens,
                                                    },
                                                }))
                                            }
                                            SseData::ContentBlockStart {
                                                content_block:
                                                    ApiContentBlock::ToolUse { id, name, .. },
                                                ..
                                            } => Some(Ok(StreamEvent::ToolUseStart { id, name })),
                                            SseData::ContentBlockStart { .. } => None,
                                            SseData::ContentBlockDelta { delta, .. } => match delta
                                            {
                                                SseDelta::TextDelta { text } => {
                                                    Some(Ok(StreamEvent::TextDelta { text }))
                                                }
                                                SseDelta::InputJsonDelta { partial_json } => {
                                                    Some(Ok(StreamEvent::ToolInputDelta {
                                                        json: partial_json,
                                                    }))
                                                }
                                            },
                                            SseData::ContentBlockStop { .. } => {
                                                Some(Ok(StreamEvent::BlockStop))
                                            }
                                            SseData::MessageDelta { delta, usage } => {
                                                Some(Ok(StreamEvent::MessageDone {
                                                    stop_reason: parse_stop_reason(
                                                        delta.stop_reason.as_deref(),
                                                    ),
                                                    usage: Usage {
                                                        input_tokens: usage.input_tokens,
                                                        output_tokens: usage.output_tokens,
                                                    },
                                                }))
                                            }
                                            SseData::MessageStop => None,
                                            SseData::Ping => None,
                                            SseData::Error { error } => Some(Err(anyhow::anyhow!(
                                                "Anthropic stream error: {}",
                                                error.message
                                            ))),
                                        };
                                        if let Some(event) = event {
                                            return Some((
                                                event,
                                                (byte_stream, buffer, current_event_type),
                                            ));
                                        }
                                        continue;
                                    }
                                    Err(e) => {
                                        debug!(error = %e, data = data, "failed to parse SSE data");
                                        continue;
                                    }
                                }
                            }

                            // Empty line or other line; continue.
                            continue;
                        }

                        // Need more data from the network.
                        match byte_stream.next().await {
                            Some(Ok(bytes)) => {
                                buffer.push_str(&String::from_utf8_lossy(&bytes));
                            }
                            Some(Err(e)) => {
                                return Some((
                                    Err(anyhow::anyhow!("stream read error: {e}")),
                                    (byte_stream, buffer, current_event_type),
                                ));
                            }
                            None => {
                                // Stream ended.
                                return None;
                            }
                        }
                    }
                },
            )
        };

        Ok(Box::pin(event_stream))
    }

    fn tool_calling_support(&self) -> ToolCallingSupport {
        ToolCallingSupport::Native
    }

    fn name(&self) -> &str {
        "anthropic"
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        // Hardcoded for now; could query API later.
        Ok(vec![
            ModelInfo {
                id: "claude-sonnet-4-20250514".into(),
                name: "Claude Sonnet 4".into(),
                context_window: 200_000,
            },
            ModelInfo {
                id: "claude-haiku-4-20250414".into(),
                name: "Claude Haiku 4".into(),
                context_window: 200_000,
            },
        ])
    }
}
