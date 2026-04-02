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

const DEFAULT_API_BASE: &str = "https://api.openai.com/v1";

// ---------------------------------------------------------------------------
// OpenAI API request/response types (private)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ApiRequest {
    model: String,
    messages: Vec<ApiMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ApiTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
}

#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ApiMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ApiToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ApiToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: ApiFunction,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ApiFunction {
    name: String,
    arguments: String,
}

#[derive(Serialize)]
struct ApiTool {
    #[serde(rename = "type")]
    tool_type: String,
    function: ApiToolFunction,
}

#[derive(Serialize)]
struct ApiToolFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Deserialize, Debug)]
struct ApiResponse {
    choices: Vec<ApiChoice>,
    #[serde(default)]
    usage: Option<ApiUsage>,
}

#[derive(Deserialize, Debug)]
struct ApiChoice {
    message: ApiResponseMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize, Debug)]
struct ApiResponseMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ApiToolCall>>,
}

#[derive(Deserialize, Debug, Clone, Default)]
struct ApiUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
}

// ---------------------------------------------------------------------------
// SSE streaming types
// ---------------------------------------------------------------------------

#[derive(Deserialize, Debug)]
struct SseChunk {
    choices: Vec<SseChoice>,
    #[serde(default)]
    usage: Option<ApiUsage>,
}

#[derive(Deserialize, Debug)]
struct SseChoice {
    delta: SseDelta,
    finish_reason: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
struct SseDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<SseToolCallDelta>>,
}

#[derive(Deserialize, Debug)]
struct SseToolCallDelta {
    #[serde(default)]
    #[allow(dead_code)]
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<SseFunctionDelta>,
}

#[derive(Deserialize, Debug)]
struct SseFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

fn to_api_messages(system: &Option<String>, messages: &[Message]) -> Vec<ApiMessage> {
    let mut api_msgs = Vec::new();

    // System message.
    if let Some(sys) = system {
        api_msgs.push(ApiMessage {
            role: "system".into(),
            content: Some(serde_json::Value::String(sys.clone())),
            tool_calls: None,
            tool_call_id: None,
        });
    }

    for msg in messages {
        match msg.role {
            Role::User => {
                // Check if this is a tool result message.
                let has_tool_results = msg
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolResult { .. }));

                if has_tool_results {
                    for block in &msg.content {
                        if let ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } = block
                        {
                            let content_str = if is_error == &Some(true) {
                                format!("Error: {content}")
                            } else {
                                content.clone()
                            };
                            api_msgs.push(ApiMessage {
                                role: "tool".into(),
                                content: Some(serde_json::Value::String(content_str)),
                                tool_calls: None,
                                tool_call_id: Some(tool_use_id.clone()),
                            });
                        }
                    }
                } else {
                    api_msgs.push(ApiMessage {
                        role: "user".into(),
                        content: Some(serde_json::Value::String(msg.text())),
                        tool_calls: None,
                        tool_call_id: None,
                    });
                }
            }
            Role::Assistant => {
                let text = msg.text();
                let tool_uses: Vec<&ContentBlock> = msg.tool_uses();

                let content = if text.is_empty() {
                    None
                } else {
                    Some(serde_json::Value::String(text))
                };

                let tool_calls = if tool_uses.is_empty() {
                    None
                } else {
                    Some(
                        tool_uses
                            .iter()
                            .map(|b| {
                                if let ContentBlock::ToolUse { id, name, input } = b {
                                    ApiToolCall {
                                        id: id.clone(),
                                        call_type: "function".into(),
                                        function: ApiFunction {
                                            name: name.clone(),
                                            arguments: serde_json::to_string(input)
                                                .unwrap_or_default(),
                                        },
                                    }
                                } else {
                                    unreachable!()
                                }
                            })
                            .collect(),
                    )
                };

                api_msgs.push(ApiMessage {
                    role: "assistant".into(),
                    content,
                    tool_calls,
                    tool_call_id: None,
                });
            }
        }
    }

    api_msgs
}

fn to_api_tools(tools: &[ToolDefinition]) -> Vec<ApiTool> {
    tools
        .iter()
        .map(|t| ApiTool {
            tool_type: "function".into(),
            function: ApiToolFunction {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.input_schema.clone(),
            },
        })
        .collect()
}

fn parse_stop_reason(reason: Option<&str>) -> StopReason {
    match reason {
        Some("tool_calls") => StopReason::ToolUse,
        Some("length") => StopReason::MaxTokens,
        _ => StopReason::EndTurn,
    }
}

// ---------------------------------------------------------------------------
// OpenAI Provider
// ---------------------------------------------------------------------------

/// OpenAI Chat Completions API provider.
pub struct OpenAiProvider {
    client: Client,
    api_key: String,
    api_base: String,
}

impl OpenAiProvider {
    /// Create a new provider with the given API key and default base URL.
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            api_base: DEFAULT_API_BASE.to_string(),
        }
    }

    /// Create with a custom API base URL (useful for proxies or Ollama).
    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.api_base = base_url;
        self
    }

    /// Create from the `OPENAI_API_KEY` environment variable.
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .context("OPENAI_API_KEY environment variable not set")?;
        let api_base = std::env::var("OPENAI_API_BASE").unwrap_or(DEFAULT_API_BASE.to_string());
        Ok(Self {
            client: Client::new(),
            api_key,
            api_base,
        })
    }

    fn build_api_request(&self, request: &CompletionRequest, stream: bool) -> ApiRequest {
        ApiRequest {
            model: request.model.clone(),
            messages: to_api_messages(&request.system, &request.messages),
            tools: to_api_tools(&request.tools),
            max_tokens: Some(request.max_tokens),
            stream,
            stream_options: if stream {
                Some(StreamOptions {
                    include_usage: true,
                })
            } else {
                None
            },
        }
    }
}

#[async_trait]
impl Provider for OpenAiProvider {
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse> {
        let api_req = self.build_api_request(&request, false);

        let resp = self
            .client
            .post(format!("{}/chat/completions", self.api_base))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&api_req)
            .send()
            .await
            .context("failed to send request to OpenAI API")?;

        let status = resp.status();
        let body = resp.text().await?;

        if !status.is_success() {
            bail!("OpenAI API error ({}): {}", status, body);
        }

        let api_resp: ApiResponse =
            serde_json::from_str(&body).context("failed to parse OpenAI API response")?;

        let choice = api_resp
            .choices
            .first()
            .ok_or_else(|| anyhow::anyhow!("no choices in OpenAI response"))?;

        let mut content_blocks = Vec::new();

        if let Some(text) = &choice.message.content
            && !text.is_empty()
        {
            content_blocks.push(ContentBlock::text(text));
        }

        if let Some(tool_calls) = &choice.message.tool_calls {
            for tc in tool_calls {
                let input: serde_json::Value = serde_json::from_str(&tc.function.arguments)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                content_blocks.push(ContentBlock::tool_use(&tc.id, &tc.function.name, input));
            }
        }

        let usage = api_resp.usage.unwrap_or_default();

        Ok(CompletionResponse {
            message: Message {
                role: Role::Assistant,
                content: content_blocks,
            },
            stop_reason: parse_stop_reason(choice.finish_reason.as_deref()),
            usage: Usage {
                input_tokens: usage.prompt_tokens,
                output_tokens: usage.completion_tokens,
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
            .post(format!("{}/chat/completions", self.api_base))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&api_req)
            .send()
            .await
            .context("failed to send streaming request to OpenAI API")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await?;
            bail!("OpenAI API error ({}): {}", status, body);
        }

        let byte_stream = resp.bytes_stream();

        let event_stream = {
            let buffer = String::new();

            stream::unfold(
                (byte_stream, buffer),
                |(mut byte_stream, mut buffer)| async move {
                    loop {
                        if let Some(newline_pos) = buffer.find('\n') {
                            let line = buffer[..newline_pos].trim_end_matches('\r').to_string();
                            buffer = buffer[newline_pos + 1..].to_string();

                            if let Some(data) = line.strip_prefix("data: ") {
                                if data == "[DONE]" {
                                    return None;
                                }

                                match serde_json::from_str::<SseChunk>(data) {
                                    Ok(chunk) => {
                                        // Handle usage in the final chunk.
                                        if let Some(usage) = &chunk.usage
                                            && chunk.choices.is_empty()
                                        {
                                            return Some((
                                                Ok(StreamEvent::MessageDone {
                                                    stop_reason: StopReason::EndTurn,
                                                    usage: Usage {
                                                        input_tokens: usage.prompt_tokens,
                                                        output_tokens: usage.completion_tokens,
                                                    },
                                                }),
                                                (byte_stream, buffer),
                                            ));
                                        }

                                        if let Some(choice) = chunk.choices.first() {
                                            // Text content delta.
                                            if let Some(text) = &choice.delta.content
                                                && !text.is_empty()
                                            {
                                                return Some((
                                                    Ok(StreamEvent::TextDelta {
                                                        text: text.clone(),
                                                    }),
                                                    (byte_stream, buffer),
                                                ));
                                            }

                                            // Tool call deltas.
                                            if let Some(tool_calls) = &choice.delta.tool_calls {
                                                for tc in tool_calls {
                                                    if let Some(id) = &tc.id {
                                                        let name = tc
                                                            .function
                                                            .as_ref()
                                                            .and_then(|f| f.name.clone())
                                                            .unwrap_or_default();
                                                        return Some((
                                                            Ok(StreamEvent::ToolUseStart {
                                                                id: id.clone(),
                                                                name,
                                                            }),
                                                            (byte_stream, buffer),
                                                        ));
                                                    }

                                                    if let Some(func) = &tc.function
                                                        && let Some(args) = &func.arguments
                                                        && !args.is_empty()
                                                    {
                                                        return Some((
                                                            Ok(StreamEvent::ToolInputDelta {
                                                                json: args.clone(),
                                                            }),
                                                            (byte_stream, buffer),
                                                        ));
                                                    }
                                                }
                                            }

                                            // Finish reason.
                                            if let Some(reason) = &choice.finish_reason {
                                                let stop = parse_stop_reason(Some(reason.as_str()));
                                                let usage = chunk.usage.unwrap_or_default();
                                                return Some((
                                                    Ok(StreamEvent::MessageDone {
                                                        stop_reason: stop,
                                                        usage: Usage {
                                                            input_tokens: usage.prompt_tokens,
                                                            output_tokens: usage.completion_tokens,
                                                        },
                                                    }),
                                                    (byte_stream, buffer),
                                                ));
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        debug!(error = %e, data = data, "failed to parse SSE chunk");
                                    }
                                }
                            }

                            continue;
                        }

                        match byte_stream.next().await {
                            Some(Ok(bytes)) => {
                                buffer.push_str(&String::from_utf8_lossy(&bytes));
                            }
                            Some(Err(e)) => {
                                return Some((
                                    Err(anyhow::anyhow!("stream read error: {e}")),
                                    (byte_stream, buffer),
                                ));
                            }
                            None => return None,
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
        "openai"
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        #[derive(Deserialize)]
        struct ApiModel {
            id: String,
        }
        #[derive(Deserialize)]
        struct ApiModelList {
            data: Vec<ApiModel>,
        }

        let url = format!("{}/models", self.api_base);
        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await
            .context("Failed to fetch OpenAI models")?;

        if !resp.status().is_success() {
            debug!("Failed to fetch models from API, using fallback list");
            return Ok(Self::fallback_models());
        }

        let list: ApiModelList = resp.json().await.context("Failed to parse model list")?;

        // Filter to chat-capable models and sort alphabetically
        let mut models: Vec<ModelInfo> = list
            .data
            .into_iter()
            .filter(|m| Self::is_chat_model(&m.id))
            .map(|m| {
                let context_window = Self::context_window_for(&m.id);
                let name = m.id.clone();
                ModelInfo {
                    id: m.id,
                    name,
                    context_window,
                }
            })
            .collect();

        models.sort_by(|a, b| a.id.cmp(&b.id));

        if models.is_empty() {
            return Ok(Self::fallback_models());
        }

        Ok(models)
    }
}

impl OpenAiProvider {
    /// Filter for models that are useful for chat/coding tasks
    fn is_chat_model(id: &str) -> bool {
        // Include gpt-4*, gpt-3.5*, o1*, o3*, o4* models
        // Exclude embeddings, tts, whisper, dall-e, moderation, etc.
        let dominated_by_chat = id.starts_with("gpt-4")
            || id.starts_with("gpt-3.5")
            || id.starts_with("o1")
            || id.starts_with("o3")
            || id.starts_with("o4")
            || id.starts_with("chatgpt");

        let is_excluded = id.contains("realtime")
            || id.contains("audio")
            || id.contains("search")
            || id.ends_with("-instruct");

        dominated_by_chat && !is_excluded
    }

    /// Rough context window estimates per model family
    fn context_window_for(id: &str) -> u32 {
        if id.starts_with("o3") || id.starts_with("o4") || id.starts_with("o1") {
            200_000
        } else if id.contains("gpt-4o") || id.contains("gpt-4.1") {
            128_000
        } else if id.starts_with("chatgpt") {
            128_000
        } else if id.contains("gpt-4-turbo") {
            128_000
        } else if id.starts_with("gpt-4") {
            8_192
        } else if id.starts_with("gpt-3.5") {
            16_385
        } else {
            128_000
        }
    }

    /// Static fallback list if the API call fails
    fn fallback_models() -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "o3".into(),
                name: "o3".into(),
                context_window: 200_000,
            },
            ModelInfo {
                id: "o4-mini".into(),
                name: "o4-mini".into(),
                context_window: 200_000,
            },
            ModelInfo {
                id: "gpt-4.1".into(),
                name: "gpt-4.1".into(),
                context_window: 128_000,
            },
            ModelInfo {
                id: "gpt-4.1-mini".into(),
                name: "gpt-4.1-mini".into(),
                context_window: 128_000,
            },
            ModelInfo {
                id: "gpt-4o".into(),
                name: "gpt-4o".into(),
                context_window: 128_000,
            },
            ModelInfo {
                id: "gpt-4o-mini".into(),
                name: "gpt-4o-mini".into(),
                context_window: 128_000,
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // parse_stop_reason
    // -----------------------------------------------------------------------

    #[test]
    fn parse_stop_reason_stop() {
        assert_eq!(parse_stop_reason(Some("stop")), StopReason::EndTurn);
    }

    #[test]
    fn parse_stop_reason_tool_calls() {
        assert_eq!(parse_stop_reason(Some("tool_calls")), StopReason::ToolUse);
    }

    #[test]
    fn parse_stop_reason_length() {
        assert_eq!(parse_stop_reason(Some("length")), StopReason::MaxTokens);
    }

    #[test]
    fn parse_stop_reason_none() {
        assert_eq!(parse_stop_reason(None), StopReason::EndTurn);
    }

    // -----------------------------------------------------------------------
    // to_api_messages
    // -----------------------------------------------------------------------

    #[test]
    fn converts_system_message() {
        let msgs = to_api_messages(&Some("Be helpful.".into()), &[]);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "system");
        assert_eq!(
            msgs[0].content,
            Some(serde_json::Value::String("Be helpful.".into()))
        );
    }

    #[test]
    fn converts_user_message() {
        let msgs = to_api_messages(&None, &[Message::user("hello")]);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "user");
    }

    #[test]
    fn converts_assistant_with_tool_calls() {
        let msg = Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::text("Let me check."),
                ContentBlock::tool_use("call_1", "file_read", serde_json::json!({"path": "a.txt"})),
            ],
        };
        let msgs = to_api_messages(&None, &[msg]);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "assistant");
        assert!(msgs[0].tool_calls.is_some());
        let tcs = msgs[0].tool_calls.as_ref().unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0].id, "call_1");
        assert_eq!(tcs[0].function.name, "file_read");
    }

    #[test]
    fn converts_tool_result_messages() {
        let msg = Message::tool_results(vec![ContentBlock::tool_result(
            "call_1",
            "file contents",
            false,
        )]);
        let msgs = to_api_messages(&None, &[msg]);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "tool");
        assert_eq!(msgs[0].tool_call_id, Some("call_1".into()));
    }

    // -----------------------------------------------------------------------
    // to_api_tools
    // -----------------------------------------------------------------------

    #[test]
    fn converts_tool_definitions() {
        let tools = vec![ToolDefinition {
            name: "test".into(),
            description: "A test tool".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }];
        let api_tools = to_api_tools(&tools);
        assert_eq!(api_tools.len(), 1);
        assert_eq!(api_tools[0].tool_type, "function");
        assert_eq!(api_tools[0].function.name, "test");
    }

    // -----------------------------------------------------------------------
    // build_api_request
    // -----------------------------------------------------------------------

    #[test]
    fn build_request_non_streaming() {
        let provider = OpenAiProvider::new("test-key".into());
        let request = CompletionRequest {
            model: "gpt-4o".into(),
            system: Some("Be helpful.".into()),
            messages: vec![Message::user("hello")],
            tools: vec![],
            max_tokens: 1024,
        };
        let api_req = provider.build_api_request(&request, false);
        assert_eq!(api_req.model, "gpt-4o");
        assert!(!api_req.stream);
        assert!(api_req.stream_options.is_none());
        // system + user = 2 messages
        assert_eq!(api_req.messages.len(), 2);
    }

    #[test]
    fn build_request_streaming_includes_usage() {
        let provider = OpenAiProvider::new("test-key".into());
        let request = CompletionRequest {
            model: "gpt-4o".into(),
            system: None,
            messages: vec![],
            tools: vec![],
            max_tokens: 4096,
        };
        let api_req = provider.build_api_request(&request, true);
        assert!(api_req.stream);
        assert!(api_req.stream_options.is_some());
        assert!(api_req.stream_options.unwrap().include_usage);
    }

    // -----------------------------------------------------------------------
    // SSE chunk deserialization
    // -----------------------------------------------------------------------

    #[test]
    fn sse_text_delta() {
        let json = r#"{"choices":[{"delta":{"content":"Hello"},"finish_reason":null}]}"#;
        let chunk: SseChunk = serde_json::from_str(json).unwrap();
        assert_eq!(chunk.choices[0].delta.content, Some("Hello".into()));
    }

    #[test]
    fn sse_tool_call_start() {
        let json = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_abc","function":{"name":"file_read","arguments":""}}]},"finish_reason":null}]}"#;
        let chunk: SseChunk = serde_json::from_str(json).unwrap();
        let tcs = chunk.choices[0].delta.tool_calls.as_ref().unwrap();
        assert_eq!(tcs[0].id, Some("call_abc".into()));
        assert_eq!(
            tcs[0].function.as_ref().unwrap().name,
            Some("file_read".into())
        );
    }

    #[test]
    fn sse_tool_call_argument_delta() {
        let json = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\":"}}]},"finish_reason":null}]}"#;
        let chunk: SseChunk = serde_json::from_str(json).unwrap();
        let tcs = chunk.choices[0].delta.tool_calls.as_ref().unwrap();
        assert_eq!(
            tcs[0].function.as_ref().unwrap().arguments,
            Some("{\"path\":".into())
        );
    }

    #[test]
    fn sse_finish_reason() {
        let json = r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#;
        let chunk: SseChunk = serde_json::from_str(json).unwrap();
        assert_eq!(chunk.choices[0].finish_reason, Some("stop".into()));
    }

    #[test]
    fn sse_usage_chunk() {
        let json = r#"{"choices":[],"usage":{"prompt_tokens":50,"completion_tokens":30}}"#;
        let chunk: SseChunk = serde_json::from_str(json).unwrap();
        let usage = chunk.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 50);
        assert_eq!(usage.completion_tokens, 30);
    }

    // -----------------------------------------------------------------------
    // API response deserialization
    // -----------------------------------------------------------------------

    #[test]
    fn api_response_text_only() {
        let json = r#"{"choices":[{"message":{"content":"Hello!"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#;
        let resp: ApiResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.choices.len(), 1);
        assert_eq!(resp.choices[0].message.content, Some("Hello!".into()));
    }

    #[test]
    fn api_response_with_tool_calls() {
        let json = r#"{"choices":[{"message":{"content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"test","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#;
        let resp: ApiResponse = serde_json::from_str(json).unwrap();
        let tcs = resp.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0].function.name, "test");
    }

    // -----------------------------------------------------------------------
    // Provider metadata
    // -----------------------------------------------------------------------

    #[test]
    fn provider_name() {
        let provider = OpenAiProvider::new("key".into());
        assert_eq!(provider.name(), "openai");
    }

    #[test]
    fn provider_tool_calling_support() {
        let provider = OpenAiProvider::new("key".into());
        assert_eq!(provider.tool_calling_support(), ToolCallingSupport::Native);
    }

    #[tokio::test]
    async fn provider_list_models() {
        let provider = OpenAiProvider::new("key".into());
        let models = provider.list_models().await.unwrap();
        assert!(!models.is_empty());
        assert!(models.iter().any(|m| m.id.contains("gpt-4o")));
    }

    #[test]
    fn from_env_fails_without_key() {
        let original = std::env::var("OPENAI_API_KEY").ok();
        unsafe { std::env::remove_var("OPENAI_API_KEY") };

        let result = OpenAiProvider::from_env();
        assert!(result.is_err());

        if let Some(key) = original {
            unsafe { std::env::set_var("OPENAI_API_KEY", key) };
        }
    }

    #[test]
    fn custom_base_url() {
        let provider =
            OpenAiProvider::new("key".into()).with_base_url("http://localhost:11434/v1".into());
        assert_eq!(provider.api_base, "http://localhost:11434/v1");
    }
}
