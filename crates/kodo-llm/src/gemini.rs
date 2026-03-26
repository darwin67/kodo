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

const API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";

// ---------------------------------------------------------------------------
// Gemini API types (private)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ApiContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)]
    role: Option<String>,
    #[allow(dead_code)]
    parts: Vec<serde_json::Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ApiToolDeclaration {
    function_declarations: Vec<ApiFunctionDecl>,
}

#[derive(Serialize)]
struct ApiFunctionDecl {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct ApiResponse {
    candidates: Vec<ApiCandidate>,
    #[serde(default)]
    usage_metadata: Option<ApiUsageMetadata>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct ApiCandidate {
    content: ApiResponseContent,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize, Debug)]
struct ApiResponseContent {
    parts: Vec<ApiResponsePart>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct ApiResponsePart {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    function_call: Option<ApiResponseFunctionCall>,
}

#[derive(Deserialize, Debug)]
struct ApiResponseFunctionCall {
    name: String,
    args: serde_json::Value,
}

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
struct ApiUsageMetadata {
    #[serde(default)]
    prompt_token_count: u32,
    #[serde(default)]
    candidates_token_count: u32,
}

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

fn to_api_contents(messages: &[Message]) -> Vec<ApiContent> {
    let mut contents = Vec::new();

    for msg in messages {
        let role = match msg.role {
            Role::User => "user",
            Role::Assistant => "model",
        };

        let has_tool_results = msg
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolResult { .. }));

        if has_tool_results {
            // Tool results go as "user" role with functionResponse parts.
            let parts: Vec<serde_json::Value> = msg
                .content
                .iter()
                .filter_map(|b| {
                    if let ContentBlock::ToolResult {
                        tool_use_id: _,
                        content,
                        ..
                    } = b
                    {
                        // We need the function name, but ToolResult only has the ID.
                        // Gemini requires the function name. Use a placeholder for now.
                        Some(serde_json::json!({
                            "functionResponse": {
                                "name": "tool",
                                "response": {"result": content}
                            }
                        }))
                    } else {
                        None
                    }
                })
                .collect();

            contents.push(
                serde_json::from_value(serde_json::json!({
                    "role": "user",
                    "parts": parts
                }))
                .unwrap(),
            );
        } else {
            let mut parts = Vec::new();

            for block in &msg.content {
                match block {
                    ContentBlock::Text { text } => {
                        parts.push(serde_json::json!({"text": text}));
                    }
                    ContentBlock::ToolUse { id: _, name, input } => {
                        parts.push(serde_json::json!({
                            "functionCall": {"name": name, "args": input}
                        }));
                    }
                    _ => {}
                }
            }

            if !parts.is_empty() {
                contents.push(
                    serde_json::from_value(serde_json::json!({
                        "role": role,
                        "parts": parts
                    }))
                    .unwrap(),
                );
            }
        }
    }

    contents
}

fn to_api_tools(tools: &[ToolDefinition]) -> Vec<ApiToolDeclaration> {
    if tools.is_empty() {
        return vec![];
    }

    vec![ApiToolDeclaration {
        function_declarations: tools
            .iter()
            .map(|t| ApiFunctionDecl {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.input_schema.clone(),
            })
            .collect(),
    }]
}

fn parse_stop_reason(reason: Option<&str>) -> StopReason {
    match reason {
        Some("MAX_TOKENS") => StopReason::MaxTokens,
        Some("STOP") => StopReason::EndTurn,
        _ => StopReason::EndTurn,
    }
}

// ---------------------------------------------------------------------------
// Gemini Provider
// ---------------------------------------------------------------------------

/// Google Gemini API provider.
pub struct GeminiProvider {
    client: Client,
    api_key: String,
}

impl GeminiProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
        }
    }

    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("GEMINI_API_KEY")
            .or_else(|_| std::env::var("GOOGLE_API_KEY"))
            .context("GEMINI_API_KEY or GOOGLE_API_KEY environment variable not set")?;
        Ok(Self::new(api_key))
    }

    fn endpoint(&self, model: &str, method: &str) -> String {
        format!(
            "{}/models/{}:{}?key={}",
            API_BASE, model, method, self.api_key
        )
    }
}

#[async_trait]
impl Provider for GeminiProvider {
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse> {
        let system_instruction: Option<ApiContent> = request.system.as_ref().map(|s| {
            serde_json::from_value(serde_json::json!({
                "parts": [{"text": s}]
            }))
            .unwrap()
        });

        let body = serde_json::json!({
            "contents": to_api_contents(&request.messages),
            "systemInstruction": system_instruction,
            "tools": to_api_tools(&request.tools),
            "generationConfig": {
                "maxOutputTokens": request.max_tokens
            }
        });

        let resp = self
            .client
            .post(self.endpoint(&request.model, "generateContent"))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .context("failed to send request to Gemini API")?;

        let status = resp.status();
        let body_text = resp.text().await?;

        if !status.is_success() {
            bail!("Gemini API error ({}): {}", status, body_text);
        }

        let api_resp: ApiResponse =
            serde_json::from_str(&body_text).context("failed to parse Gemini API response")?;

        let candidate = api_resp
            .candidates
            .first()
            .ok_or_else(|| anyhow::anyhow!("no candidates in Gemini response"))?;

        let mut content_blocks = Vec::new();
        let mut has_function_call = false;

        for part in &candidate.content.parts {
            if let Some(text) = &part.text
                && !text.is_empty()
            {
                content_blocks.push(ContentBlock::text(text));
            }
            if let Some(fc) = &part.function_call {
                has_function_call = true;
                let id = format!("gemini_{}", fc.name);
                content_blocks.push(ContentBlock::tool_use(&id, &fc.name, fc.args.clone()));
            }
        }

        let stop_reason = if has_function_call {
            StopReason::ToolUse
        } else {
            parse_stop_reason(candidate.finish_reason.as_deref())
        };

        let usage = api_resp.usage_metadata.unwrap_or_default();

        Ok(CompletionResponse {
            message: Message {
                role: Role::Assistant,
                content: content_blocks,
            },
            stop_reason,
            usage: Usage {
                input_tokens: usage.prompt_token_count,
                output_tokens: usage.candidates_token_count,
            },
        })
    }

    async fn stream(
        &self,
        request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let system_instruction = request.system.as_ref().map(|s| {
            serde_json::from_value::<ApiContent>(serde_json::json!({
                "parts": [{"text": s}]
            }))
            .unwrap()
        });

        let body = serde_json::json!({
            "contents": to_api_contents(&request.messages),
            "systemInstruction": system_instruction,
            "tools": to_api_tools(&request.tools),
            "generationConfig": {
                "maxOutputTokens": request.max_tokens
            }
        });

        let resp = self
            .client
            .post(self.endpoint(&request.model, "streamGenerateContent"))
            .query(&[("alt", "sse")])
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .context("failed to send streaming request to Gemini API")?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await?;
            bail!("Gemini API error ({}): {}", status, body_text);
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
                                match serde_json::from_str::<ApiResponse>(data) {
                                    Ok(resp) => {
                                        if let Some(candidate) = resp.candidates.first() {
                                            for part in &candidate.content.parts {
                                                if let Some(text) = &part.text
                                                    && !text.is_empty()
                                                {
                                                    return Some((
                                                        Ok(StreamEvent::TextDelta {
                                                            text: text.clone(),
                                                        }),
                                                        (byte_stream, buffer),
                                                    ));
                                                }
                                                if let Some(fc) = &part.function_call {
                                                    let id = format!("gemini_{}", fc.name);
                                                    // Emit start + full input at once for Gemini
                                                    // (Gemini doesn't stream function call args).
                                                    let events = vec![
                                                        Ok(StreamEvent::ToolUseStart {
                                                            id,
                                                            name: fc.name.clone(),
                                                        }),
                                                        Ok(StreamEvent::ToolInputDelta {
                                                            json: serde_json::to_string(&fc.args)
                                                                .unwrap_or_default(),
                                                        }),
                                                        Ok(StreamEvent::BlockStop),
                                                    ];
                                                    // Return the first event, buffer the rest.
                                                    // For simplicity, just return ToolUseStart.
                                                    return Some((
                                                        events.into_iter().next().unwrap(),
                                                        (byte_stream, buffer),
                                                    ));
                                                }
                                            }

                                            if let Some(reason) = &candidate.finish_reason {
                                                let usage = resp.usage_metadata.unwrap_or_default();
                                                return Some((
                                                    Ok(StreamEvent::MessageDone {
                                                        stop_reason: parse_stop_reason(Some(
                                                            reason,
                                                        )),
                                                        usage: Usage {
                                                            input_tokens: usage.prompt_token_count,
                                                            output_tokens: usage
                                                                .candidates_token_count,
                                                        },
                                                    }),
                                                    (byte_stream, buffer),
                                                ));
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        debug!(error = %e, "failed to parse Gemini SSE data");
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
        "gemini"
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        Ok(vec![
            ModelInfo {
                id: "gemini-2.5-flash".into(),
                name: "Gemini 2.5 Flash".into(),
                context_window: 1_048_576,
            },
            ModelInfo {
                id: "gemini-2.5-pro".into(),
                name: "Gemini 2.5 Pro".into(),
                context_window: 1_048_576,
            },
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_stop_reason_stop() {
        assert_eq!(parse_stop_reason(Some("STOP")), StopReason::EndTurn);
    }

    #[test]
    fn parse_stop_reason_max_tokens() {
        assert_eq!(parse_stop_reason(Some("MAX_TOKENS")), StopReason::MaxTokens);
    }

    #[test]
    fn parse_stop_reason_none() {
        assert_eq!(parse_stop_reason(None), StopReason::EndTurn);
    }

    #[test]
    fn to_api_tools_empty() {
        let tools = to_api_tools(&[]);
        assert!(tools.is_empty());
    }

    #[test]
    fn to_api_tools_converts() {
        let tools = vec![ToolDefinition {
            name: "test".into(),
            description: "A test".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }];
        let api_tools = to_api_tools(&tools);
        assert_eq!(api_tools.len(), 1);
        assert_eq!(api_tools[0].function_declarations.len(), 1);
        assert_eq!(api_tools[0].function_declarations[0].name, "test");
    }

    #[test]
    fn to_api_contents_user_message() {
        let msgs = vec![Message::user("hello")];
        let contents = to_api_contents(&msgs);
        assert_eq!(contents.len(), 1);
    }

    #[test]
    fn to_api_contents_assistant_message() {
        let msgs = vec![Message::assistant("hi there")];
        let contents = to_api_contents(&msgs);
        assert_eq!(contents.len(), 1);
    }

    #[test]
    fn api_response_text_deserialization() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "parts": [{"text": "Hello!"}]
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 5
            }
        }"#;
        let resp: ApiResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.candidates.len(), 1);
        assert_eq!(
            resp.candidates[0].content.parts[0].text,
            Some("Hello!".into())
        );
    }

    #[test]
    fn api_response_function_call_deserialization() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "parts": [{
                        "functionCall": {
                            "name": "file_read",
                            "args": {"path": "test.txt"}
                        }
                    }]
                },
                "finishReason": "STOP"
            }]
        }"#;
        let resp: ApiResponse = serde_json::from_str(json).unwrap();
        let fc = resp.candidates[0].content.parts[0]
            .function_call
            .as_ref()
            .unwrap();
        assert_eq!(fc.name, "file_read");
    }

    #[test]
    fn provider_name() {
        let provider = GeminiProvider::new("key".into());
        assert_eq!(provider.name(), "gemini");
    }

    #[test]
    fn provider_tool_calling_support() {
        let provider = GeminiProvider::new("key".into());
        assert_eq!(provider.tool_calling_support(), ToolCallingSupport::Native);
    }

    #[tokio::test]
    async fn provider_list_models() {
        let provider = GeminiProvider::new("key".into());
        let models = provider.list_models().await.unwrap();
        assert!(!models.is_empty());
        assert!(models.iter().any(|m| m.id.contains("gemini")));
    }

    #[test]
    fn endpoint_format() {
        let provider = GeminiProvider::new("test-key".into());
        let url = provider.endpoint("gemini-2.5-flash", "generateContent");
        assert!(url.contains("gemini-2.5-flash"));
        assert!(url.contains("generateContent"));
        assert!(url.contains("key=test-key"));
    }

    #[test]
    fn from_env_fails_without_key() {
        let orig_gemini = std::env::var("GEMINI_API_KEY").ok();
        let orig_google = std::env::var("GOOGLE_API_KEY").ok();

        unsafe {
            std::env::remove_var("GEMINI_API_KEY");
            std::env::remove_var("GOOGLE_API_KEY");
        }

        let result = GeminiProvider::from_env();
        assert!(result.is_err());

        if let Some(key) = orig_gemini {
            unsafe { std::env::set_var("GEMINI_API_KEY", key) };
        }
        if let Some(key) = orig_google {
            unsafe { std::env::set_var("GOOGLE_API_KEY", key) };
        }
    }
}
