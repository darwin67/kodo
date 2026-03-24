use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Roles & Messages
// ---------------------------------------------------------------------------

/// The role of a message participant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

/// A single content block within a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
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

impl ContentBlock {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    pub fn tool_use(
        id: impl Into<String>,
        name: impl Into<String>,
        input: serde_json::Value,
    ) -> Self {
        Self::ToolUse {
            id: id.into(),
            name: name.into(),
            input,
        }
    }

    pub fn tool_result(
        tool_use_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self::ToolResult {
            tool_use_id: tool_use_id.into(),
            content: content.into(),
            is_error: if is_error { Some(true) } else { None },
        }
    }

    /// Extract the text content if this is a Text block.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            _ => None,
        }
    }
}

/// A message in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl Message {
    /// Create a user message with a single text block.
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::text(text)],
        }
    }

    /// Create an assistant message with a single text block.
    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentBlock::text(text)],
        }
    }

    /// Create a user message containing tool results.
    pub fn tool_results(results: Vec<ContentBlock>) -> Self {
        Self {
            role: Role::User,
            content: results,
        }
    }

    /// Collect all text from Text blocks in this message.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| b.as_text())
            .collect::<Vec<_>>()
            .join("")
    }

    /// Collect all tool-use blocks from this message.
    pub fn tool_uses(&self) -> Vec<&ContentBlock> {
        self.content
            .iter()
            .filter(|b| matches!(b, ContentBlock::ToolUse { .. }))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Completion request / response
// ---------------------------------------------------------------------------

/// Tool definition to send to the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// A request to the LLM.
#[derive(Debug, Clone)]
pub struct CompletionRequest {
    pub model: String,
    pub system: Option<String>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
    pub max_tokens: u32,
}

/// A complete (non-streaming) response from the LLM.
#[derive(Debug, Clone)]
pub struct CompletionResponse {
    pub message: Message,
    pub stop_reason: StopReason,
    pub usage: Usage,
}

/// Why the LLM stopped generating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// Natural end of response.
    EndTurn,
    /// The model wants to call a tool.
    ToolUse,
    /// Hit the max_tokens limit.
    MaxTokens,
}

/// Token usage information.
#[derive(Debug, Clone, Copy, Default)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

// ---------------------------------------------------------------------------
// Streaming types
// ---------------------------------------------------------------------------

/// A chunk of a streaming response.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// Start of the response; includes metadata.
    MessageStart { usage: Usage },
    /// A delta of text content.
    TextDelta { text: String },
    /// Start of a tool-use block.
    ToolUseStart { id: String, name: String },
    /// A delta of tool-use input JSON.
    ToolInputDelta { json: String },
    /// A content block has finished.
    BlockStop,
    /// The full message is done.
    MessageDone {
        stop_reason: StopReason,
        usage: Usage,
    },
}

// ---------------------------------------------------------------------------
// Provider capability
// ---------------------------------------------------------------------------

/// Whether a provider supports native tool calling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallingSupport {
    /// Provider has a native tool-calling API (Anthropic, OpenAI, Gemini).
    Native,
    /// Tool calls must be parsed from text output (older/local models).
    TextBased,
    /// No tool calling support at all.
    None,
}

/// Basic metadata about a model.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub context_window: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // ContentBlock
    // -----------------------------------------------------------------------

    #[test]
    fn content_block_text_constructor() {
        let block = ContentBlock::text("hello");
        assert_eq!(block.as_text(), Some("hello"));
    }

    #[test]
    fn content_block_text_from_string() {
        let block = ContentBlock::text(String::from("world"));
        assert_eq!(block.as_text(), Some("world"));
    }

    #[test]
    fn content_block_tool_use_constructor() {
        let input = serde_json::json!({"path": "/tmp"});
        let block = ContentBlock::tool_use("id-1", "file_read", input.clone());
        match &block {
            ContentBlock::ToolUse { id, name, input: i } => {
                assert_eq!(id, "id-1");
                assert_eq!(name, "file_read");
                assert_eq!(i, &input);
            }
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn content_block_tool_result_success() {
        let block = ContentBlock::tool_result("id-1", "file contents", false);
        match &block {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "id-1");
                assert_eq!(content, "file contents");
                assert_eq!(*is_error, None);
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn content_block_tool_result_error() {
        let block = ContentBlock::tool_result("id-2", "something failed", true);
        match &block {
            ContentBlock::ToolResult { is_error, .. } => {
                assert_eq!(*is_error, Some(true));
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn content_block_as_text_returns_none_for_tool_use() {
        let block = ContentBlock::tool_use("id", "name", serde_json::json!({}));
        assert_eq!(block.as_text(), None);
    }

    #[test]
    fn content_block_as_text_returns_none_for_tool_result() {
        let block = ContentBlock::tool_result("id", "content", false);
        assert_eq!(block.as_text(), None);
    }

    // -----------------------------------------------------------------------
    // ContentBlock serialization
    // -----------------------------------------------------------------------

    #[test]
    fn content_block_text_serialization_roundtrip() {
        let block = ContentBlock::text("hello");
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains(r#""type":"text""#));
        assert!(json.contains(r#""text":"hello""#));

        let deserialized: ContentBlock = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.as_text(), Some("hello"));
    }

    #[test]
    fn content_block_tool_use_serialization_roundtrip() {
        let block = ContentBlock::tool_use("id-1", "shell", serde_json::json!({"cmd": "ls"}));
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains(r#""type":"tool_use""#));

        let deserialized: ContentBlock = serde_json::from_str(&json).unwrap();
        match deserialized {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "id-1");
                assert_eq!(name, "shell");
                assert_eq!(input, serde_json::json!({"cmd": "ls"}));
            }
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn content_block_tool_result_serialization_skips_none_is_error() {
        let block = ContentBlock::tool_result("id-1", "ok", false);
        let json = serde_json::to_string(&block).unwrap();
        // is_error should be absent (skipped when None)
        assert!(!json.contains("is_error"));
    }

    #[test]
    fn content_block_tool_result_serialization_includes_is_error_when_true() {
        let block = ContentBlock::tool_result("id-1", "fail", true);
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains(r#""is_error":true"#));
    }

    // -----------------------------------------------------------------------
    // Role serialization
    // -----------------------------------------------------------------------

    #[test]
    fn role_serialization() {
        assert_eq!(serde_json::to_string(&Role::User).unwrap(), r#""user""#);
        assert_eq!(
            serde_json::to_string(&Role::Assistant).unwrap(),
            r#""assistant""#
        );
    }

    #[test]
    fn role_deserialization() {
        let user: Role = serde_json::from_str(r#""user""#).unwrap();
        assert_eq!(user, Role::User);
        let assistant: Role = serde_json::from_str(r#""assistant""#).unwrap();
        assert_eq!(assistant, Role::Assistant);
    }

    // -----------------------------------------------------------------------
    // Message
    // -----------------------------------------------------------------------

    #[test]
    fn message_user_constructor() {
        let msg = Message::user("hello");
        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.content.len(), 1);
        assert_eq!(msg.text(), "hello");
    }

    #[test]
    fn message_assistant_constructor() {
        let msg = Message::assistant("hi there");
        assert_eq!(msg.role, Role::Assistant);
        assert_eq!(msg.text(), "hi there");
    }

    #[test]
    fn message_tool_results_constructor() {
        let results = vec![
            ContentBlock::tool_result("id-1", "result 1", false),
            ContentBlock::tool_result("id-2", "result 2", true),
        ];
        let msg = Message::tool_results(results);
        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.content.len(), 2);
    }

    #[test]
    fn message_text_concatenates_text_blocks() {
        let msg = Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::text("hello "),
                ContentBlock::tool_use("id", "name", serde_json::json!({})),
                ContentBlock::text("world"),
            ],
        };
        assert_eq!(msg.text(), "hello world");
    }

    #[test]
    fn message_text_empty_when_no_text_blocks() {
        let msg = Message {
            role: Role::Assistant,
            content: vec![ContentBlock::tool_use("id", "name", serde_json::json!({}))],
        };
        assert_eq!(msg.text(), "");
    }

    #[test]
    fn message_tool_uses_filters_correctly() {
        let msg = Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::text("thinking..."),
                ContentBlock::tool_use("id-1", "file_read", serde_json::json!({})),
                ContentBlock::tool_use("id-2", "shell", serde_json::json!({})),
            ],
        };
        let tool_uses = msg.tool_uses();
        assert_eq!(tool_uses.len(), 2);
    }

    #[test]
    fn message_tool_uses_empty_when_no_tool_blocks() {
        let msg = Message::assistant("just text");
        assert!(msg.tool_uses().is_empty());
    }

    // -----------------------------------------------------------------------
    // Message serialization roundtrip
    // -----------------------------------------------------------------------

    #[test]
    fn message_serialization_roundtrip() {
        let msg = Message::user("test input");
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.role, Role::User);
        assert_eq!(deserialized.text(), "test input");
    }

    // -----------------------------------------------------------------------
    // ToolDefinition
    // -----------------------------------------------------------------------

    #[test]
    fn tool_definition_serialization_roundtrip() {
        let def = ToolDefinition {
            name: "file_read".into(),
            description: "Read a file".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }),
        };
        let json = serde_json::to_string(&def).unwrap();
        let deserialized: ToolDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "file_read");
        assert_eq!(deserialized.description, "Read a file");
    }

    // -----------------------------------------------------------------------
    // Usage
    // -----------------------------------------------------------------------

    #[test]
    fn usage_default() {
        let usage = Usage::default();
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
    }

    // -----------------------------------------------------------------------
    // StopReason
    // -----------------------------------------------------------------------

    #[test]
    fn stop_reason_equality() {
        assert_eq!(StopReason::EndTurn, StopReason::EndTurn);
        assert_eq!(StopReason::ToolUse, StopReason::ToolUse);
        assert_eq!(StopReason::MaxTokens, StopReason::MaxTokens);
        assert_ne!(StopReason::EndTurn, StopReason::ToolUse);
    }
}
