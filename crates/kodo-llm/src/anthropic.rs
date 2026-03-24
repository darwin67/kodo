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

                            if let Some(stripped) = line.strip_prefix("event: ") {
                                current_event_type = stripped.to_string();
                                continue;
                            }

                            if let Some(data) = line.strip_prefix("data: ") {
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

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // parse_stop_reason
    // -----------------------------------------------------------------------

    #[test]
    fn parse_stop_reason_end_turn() {
        assert_eq!(parse_stop_reason(Some("end_turn")), StopReason::EndTurn);
    }

    #[test]
    fn parse_stop_reason_tool_use() {
        assert_eq!(parse_stop_reason(Some("tool_use")), StopReason::ToolUse);
    }

    #[test]
    fn parse_stop_reason_max_tokens() {
        assert_eq!(parse_stop_reason(Some("max_tokens")), StopReason::MaxTokens);
    }

    #[test]
    fn parse_stop_reason_none_defaults_to_end_turn() {
        assert_eq!(parse_stop_reason(None), StopReason::EndTurn);
    }

    #[test]
    fn parse_stop_reason_unknown_defaults_to_end_turn() {
        assert_eq!(parse_stop_reason(Some("unknown")), StopReason::EndTurn);
    }

    // -----------------------------------------------------------------------
    // to_api_content_block / from_api_content_block roundtrip
    // -----------------------------------------------------------------------

    #[test]
    fn content_block_text_roundtrip() {
        let original = ContentBlock::text("hello");
        let api = to_api_content_block(&original);
        let back = from_api_content_block(&api);
        assert_eq!(back.as_text(), Some("hello"));
    }

    #[test]
    fn content_block_tool_use_roundtrip() {
        let input = serde_json::json!({"path": "/tmp/file.txt"});
        let original = ContentBlock::tool_use("tu-123", "file_read", input.clone());
        let api = to_api_content_block(&original);
        let back = from_api_content_block(&api);
        match back {
            ContentBlock::ToolUse { id, name, input: i } => {
                assert_eq!(id, "tu-123");
                assert_eq!(name, "file_read");
                assert_eq!(i, input);
            }
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn content_block_tool_result_roundtrip() {
        let original = ContentBlock::tool_result("tu-123", "file contents here", false);
        let api = to_api_content_block(&original);
        let back = from_api_content_block(&api);
        match back {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "tu-123");
                assert_eq!(content, "file contents here");
                assert_eq!(is_error, None);
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn content_block_tool_result_error_roundtrip() {
        let original = ContentBlock::tool_result("tu-456", "error msg", true);
        let api = to_api_content_block(&original);
        let back = from_api_content_block(&api);
        match back {
            ContentBlock::ToolResult { is_error, .. } => {
                assert_eq!(is_error, Some(true));
            }
            _ => panic!("expected ToolResult"),
        }
    }

    // -----------------------------------------------------------------------
    // to_api_messages
    // -----------------------------------------------------------------------

    #[test]
    fn to_api_messages_user() {
        let msgs = vec![Message::user("hello")];
        let api_msgs = to_api_messages(&msgs);
        assert_eq!(api_msgs.len(), 1);
        assert_eq!(api_msgs[0].role, "user");
        assert_eq!(api_msgs[0].content.len(), 1);
    }

    #[test]
    fn to_api_messages_assistant() {
        let msgs = vec![Message::assistant("hi there")];
        let api_msgs = to_api_messages(&msgs);
        assert_eq!(api_msgs[0].role, "assistant");
    }

    #[test]
    fn to_api_messages_preserves_order() {
        let msgs = vec![
            Message::user("first"),
            Message::assistant("second"),
            Message::user("third"),
        ];
        let api_msgs = to_api_messages(&msgs);
        assert_eq!(api_msgs.len(), 3);
        assert_eq!(api_msgs[0].role, "user");
        assert_eq!(api_msgs[1].role, "assistant");
        assert_eq!(api_msgs[2].role, "user");
    }

    // -----------------------------------------------------------------------
    // to_api_tools
    // -----------------------------------------------------------------------

    #[test]
    fn to_api_tools_converts_correctly() {
        let tools = vec![ToolDefinition {
            name: "file_read".into(),
            description: "Read a file".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                }
            }),
        }];
        let api_tools = to_api_tools(&tools);
        assert_eq!(api_tools.len(), 1);
        assert_eq!(api_tools[0].name, "file_read");
        assert_eq!(api_tools[0].description, "Read a file");
    }

    #[test]
    fn to_api_tools_empty() {
        let api_tools = to_api_tools(&[]);
        assert!(api_tools.is_empty());
    }

    // -----------------------------------------------------------------------
    // build_api_request
    // -----------------------------------------------------------------------

    #[test]
    fn build_api_request_non_streaming() {
        let provider = AnthropicProvider::new("test-key".into());
        let request = CompletionRequest {
            model: "claude-sonnet-4-20250514".into(),
            system: Some("You are helpful.".into()),
            messages: vec![Message::user("hello")],
            tools: vec![],
            max_tokens: 1024,
        };
        let api_req = provider.build_api_request(&request, false);
        assert_eq!(api_req.model, "claude-sonnet-4-20250514");
        assert_eq!(api_req.max_tokens, 1024);
        assert!(!api_req.stream);
        assert_eq!(api_req.system, Some("You are helpful.".into()));
        assert_eq!(api_req.messages.len(), 1);
        assert!(api_req.tools.is_empty());
    }

    #[test]
    fn build_api_request_streaming() {
        let provider = AnthropicProvider::new("test-key".into());
        let request = CompletionRequest {
            model: "claude-sonnet-4-20250514".into(),
            system: None,
            messages: vec![],
            tools: vec![],
            max_tokens: 4096,
        };
        let api_req = provider.build_api_request(&request, true);
        assert!(api_req.stream);
        assert_eq!(api_req.system, None);
    }

    #[test]
    fn build_api_request_serialization() {
        let provider = AnthropicProvider::new("test-key".into());
        let request = CompletionRequest {
            model: "claude-sonnet-4-20250514".into(),
            system: Some("system".into()),
            messages: vec![Message::user("hi")],
            tools: vec![ToolDefinition {
                name: "test".into(),
                description: "test tool".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }],
            max_tokens: 1024,
        };
        let api_req = provider.build_api_request(&request, false);
        // Should serialize without error
        let json = serde_json::to_string(&api_req).unwrap();
        assert!(json.contains("claude-sonnet-4-20250514"));
        assert!(json.contains(r#""stream":false"#));
    }

    #[test]
    fn build_api_request_omits_system_when_none() {
        let provider = AnthropicProvider::new("test-key".into());
        let request = CompletionRequest {
            model: "test".into(),
            system: None,
            messages: vec![],
            tools: vec![],
            max_tokens: 1024,
        };
        let api_req = provider.build_api_request(&request, false);
        let json = serde_json::to_string(&api_req).unwrap();
        // system should be absent when None
        assert!(!json.contains("system"));
    }

    #[test]
    fn build_api_request_omits_tools_when_empty() {
        let provider = AnthropicProvider::new("test-key".into());
        let request = CompletionRequest {
            model: "test".into(),
            system: None,
            messages: vec![],
            tools: vec![],
            max_tokens: 1024,
        };
        let api_req = provider.build_api_request(&request, false);
        let json = serde_json::to_string(&api_req).unwrap();
        // tools should be absent when empty
        assert!(!json.contains("tools"));
    }

    // -----------------------------------------------------------------------
    // SSE data deserialization
    // -----------------------------------------------------------------------

    #[test]
    fn sse_message_start_deserialization() {
        let data =
            r#"{"type":"message_start","message":{"usage":{"input_tokens":25,"output_tokens":0}}}"#;
        let sse: SseData = serde_json::from_str(data).unwrap();
        match sse {
            SseData::MessageStart { message } => {
                assert_eq!(message.usage.input_tokens, 25);
                assert_eq!(message.usage.output_tokens, 0);
            }
            _ => panic!("expected MessageStart"),
        }
    }

    #[test]
    fn sse_content_block_start_text_deserialization() {
        let data =
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#;
        let sse: SseData = serde_json::from_str(data).unwrap();
        match sse {
            SseData::ContentBlockStart {
                index,
                content_block,
            } => {
                assert_eq!(index, 0);
                match content_block {
                    ApiContentBlock::Text { text } => assert_eq!(text, ""),
                    _ => panic!("expected Text block"),
                }
            }
            _ => panic!("expected ContentBlockStart"),
        }
    }

    #[test]
    fn sse_content_block_start_tool_use_deserialization() {
        let data = r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tu_123","name":"file_read","input":{}}}"#;
        let sse: SseData = serde_json::from_str(data).unwrap();
        match sse {
            SseData::ContentBlockStart {
                content_block: ApiContentBlock::ToolUse { id, name, .. },
                ..
            } => {
                assert_eq!(id, "tu_123");
                assert_eq!(name, "file_read");
            }
            _ => panic!("expected ContentBlockStart with ToolUse"),
        }
    }

    #[test]
    fn sse_text_delta_deserialization() {
        let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
        let sse: SseData = serde_json::from_str(data).unwrap();
        match sse {
            SseData::ContentBlockDelta {
                delta: SseDelta::TextDelta { text },
                ..
            } => {
                assert_eq!(text, "Hello");
            }
            _ => panic!("expected ContentBlockDelta with TextDelta"),
        }
    }

    #[test]
    fn sse_input_json_delta_deserialization() {
        let data = r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#;
        let sse: SseData = serde_json::from_str(data).unwrap();
        match sse {
            SseData::ContentBlockDelta {
                delta: SseDelta::InputJsonDelta { partial_json },
                ..
            } => {
                assert_eq!(partial_json, r#"{"path":"#);
            }
            _ => panic!("expected ContentBlockDelta with InputJsonDelta"),
        }
    }

    #[test]
    fn sse_content_block_stop_deserialization() {
        let data = r#"{"type":"content_block_stop","index":0}"#;
        let sse: SseData = serde_json::from_str(data).unwrap();
        assert!(matches!(sse, SseData::ContentBlockStop { index: 0 }));
    }

    #[test]
    fn sse_message_delta_deserialization() {
        let data = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":0,"output_tokens":42}}"#;
        let sse: SseData = serde_json::from_str(data).unwrap();
        match sse {
            SseData::MessageDelta { delta, usage } => {
                assert_eq!(delta.stop_reason.as_deref(), Some("end_turn"));
                assert_eq!(usage.output_tokens, 42);
            }
            _ => panic!("expected MessageDelta"),
        }
    }

    #[test]
    fn sse_message_delta_tool_use_stop_reason() {
        let data = r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"input_tokens":0,"output_tokens":10}}"#;
        let sse: SseData = serde_json::from_str(data).unwrap();
        match sse {
            SseData::MessageDelta { delta, .. } => {
                assert_eq!(
                    parse_stop_reason(delta.stop_reason.as_deref()),
                    StopReason::ToolUse
                );
            }
            _ => panic!("expected MessageDelta"),
        }
    }

    #[test]
    fn sse_message_stop_deserialization() {
        let data = r#"{"type":"message_stop"}"#;
        let sse: SseData = serde_json::from_str(data).unwrap();
        assert!(matches!(sse, SseData::MessageStop));
    }

    #[test]
    fn sse_ping_deserialization() {
        let data = r#"{"type":"ping"}"#;
        let sse: SseData = serde_json::from_str(data).unwrap();
        assert!(matches!(sse, SseData::Ping));
    }

    #[test]
    fn sse_error_deserialization() {
        let data = r#"{"type":"error","error":{"message":"rate limited"}}"#;
        let sse: SseData = serde_json::from_str(data).unwrap();
        match sse {
            SseData::Error { error } => {
                assert_eq!(error.message, "rate limited");
            }
            _ => panic!("expected Error"),
        }
    }

    // -----------------------------------------------------------------------
    // API response deserialization
    // -----------------------------------------------------------------------

    #[test]
    fn api_response_text_deserialization() {
        let json = r#"{
            "content": [{"type": "text", "text": "Hello, world!"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 5}
        }"#;
        let resp: ApiResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.content.len(), 1);
        assert_eq!(resp.stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(resp.usage.input_tokens, 10);
        assert_eq!(resp.usage.output_tokens, 5);
    }

    #[test]
    fn api_response_tool_use_deserialization() {
        let json = r#"{
            "content": [
                {"type": "text", "text": "Let me read that file."},
                {"type": "tool_use", "id": "tu_abc", "name": "file_read", "input": {"path": "/tmp/test.txt"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 50, "output_tokens": 30}
        }"#;
        let resp: ApiResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.content.len(), 2);
        assert_eq!(resp.stop_reason.as_deref(), Some("tool_use"));
    }

    #[test]
    fn api_error_deserialization() {
        let json = r#"{"error": {"message": "Invalid API key"}}"#;
        let err: ApiError = serde_json::from_str(json).unwrap();
        assert_eq!(err.error.message, "Invalid API key");
    }

    // -----------------------------------------------------------------------
    // Provider metadata
    // -----------------------------------------------------------------------

    #[test]
    fn provider_name() {
        let provider = AnthropicProvider::new("test-key".into());
        assert_eq!(provider.name(), "anthropic");
    }

    #[test]
    fn provider_tool_calling_support() {
        let provider = AnthropicProvider::new("test-key".into());
        assert_eq!(provider.tool_calling_support(), ToolCallingSupport::Native);
    }

    #[tokio::test]
    async fn provider_list_models() {
        let provider = AnthropicProvider::new("test-key".into());
        let models = provider.list_models().await.unwrap();
        assert!(models.len() >= 2);
        assert!(models.iter().any(|m| m.id.contains("sonnet")));
        assert!(models.iter().any(|m| m.id.contains("haiku")));
        assert!(models.iter().all(|m| m.context_window > 0));
    }

    // -----------------------------------------------------------------------
    // from_env
    // -----------------------------------------------------------------------

    #[test]
    fn from_env_fails_without_key() {
        // Temporarily remove the env var if it exists
        let original = std::env::var("ANTHROPIC_API_KEY").ok();
        // SAFETY: This test does not run in parallel with other tests that
        // read ANTHROPIC_API_KEY. The env var is restored before returning.
        unsafe {
            std::env::remove_var("ANTHROPIC_API_KEY");
        }

        let result = AnthropicProvider::from_env();
        assert!(result.is_err());

        // Restore
        if let Some(key) = original {
            unsafe {
                std::env::set_var("ANTHROPIC_API_KEY", key);
            }
        }
    }
}
