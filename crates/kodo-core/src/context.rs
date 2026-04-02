use kodo_llm::types::{ContentBlock, Message, Role, Usage};
use std::collections::HashMap;

/// Model context window limits (conservative estimates).
/// These are slightly lower than the actual limits to provide buffer.
const MODEL_CONTEXT_WINDOWS: &[(&str, u32)] = &[
    // Anthropic models
    ("claude-3-5-sonnet", 180_000), // actual: 200k
    ("claude-3-5-haiku", 180_000),  // actual: 200k
    ("claude-3-opus", 180_000),     // actual: 200k
    ("claude-3-sonnet", 180_000),   // actual: 200k
    ("claude-3-haiku", 180_000),    // actual: 200k
    ("claude-2.1", 180_000),        // actual: 200k
    ("claude-2.0", 90_000),         // actual: 100k
    // New Anthropic models
    ("claude-3-5-sonnet-20241022", 180_000), // actual: 200k
    ("claude-3-5-haiku-20241022", 180_000),  // actual: 200k
    ("claude-sonnet-3.5", 180_000),          // actual: 200k
    ("claude-haiku-3", 180_000),             // actual: 200k
    ("claude-sonnet-4-20250514", 180_000),   // Kodo default model
    // OpenAI models
    ("gpt-4o", 120_000),       // actual: 128k
    ("gpt-4o-mini", 120_000),  // actual: 128k
    ("gpt-4-turbo", 120_000),  // actual: 128k
    ("gpt-4", 7_500),          // actual: 8k
    ("gpt-3.5-turbo", 15_000), // actual: 16k
    // Google models
    ("gemini-1.5-pro", 1_000_000),   // actual: 1M+
    ("gemini-1.5-flash", 1_000_000), // actual: 1M+
    ("gemini-1.0-pro", 30_000),      // actual: 32k
    // Default for unknown models
    ("default", 30_000),
];

/// Tracks token usage and manages context window limits.
#[derive(Debug)]
pub struct ContextTracker {
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    /// Current conversation tokens (approximate count).
    pub current_conversation_tokens: u32,
    /// Model context limits
    model_limits: HashMap<String, u32>,
    /// Current model's context window
    pub current_model_limit: u32,
}

impl Default for ContextTracker {
    fn default() -> Self {
        let mut model_limits = HashMap::new();
        for (model, limit) in MODEL_CONTEXT_WINDOWS {
            model_limits.insert(model.to_string(), *limit);
        }

        Self {
            total_input_tokens: 0,
            total_output_tokens: 0,
            current_conversation_tokens: 0,
            model_limits,
            current_model_limit: 30_000, // conservative default
        }
    }
}

impl ContextTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Update the current model and its context limit.
    pub fn set_model(&mut self, model: &str) {
        // Try exact match first
        if let Some(&limit) = self.model_limits.get(model) {
            self.current_model_limit = limit;
            return;
        }

        // Try prefix match (e.g., "claude-3-5-sonnet-20241022" matches "claude-3-5-sonnet")
        for (key, &limit) in &self.model_limits {
            if model.starts_with(key) || key.starts_with(model) {
                self.current_model_limit = limit;
                return;
            }
        }

        // Default fallback
        self.current_model_limit = self.model_limits["default"];
    }

    /// Record token usage from a completion.
    pub fn record(&mut self, usage: &Usage) {
        self.total_input_tokens += usage.input_tokens as u64;
        self.total_output_tokens += usage.output_tokens as u64;
    }

    /// Update the current conversation token count.
    pub fn update_conversation_tokens(
        &mut self,
        messages: &[Message],
        system_prompt: Option<&str>,
    ) {
        self.current_conversation_tokens = Self::estimate_tokens(messages, system_prompt);
    }

    /// Total tokens consumed in this session.
    pub fn total_tokens(&self) -> u64 {
        self.total_input_tokens + self.total_output_tokens
    }

    /// Check if we're approaching the context limit.
    pub fn is_nearing_limit(&self) -> bool {
        // Consider "near" as > 80% of the limit
        self.current_conversation_tokens > (self.current_model_limit * 8 / 10)
    }

    /// Get the percentage of context used.
    pub fn context_usage_percent(&self) -> f32 {
        (self.current_conversation_tokens as f32 / self.current_model_limit as f32) * 100.0
    }

    /// Get remaining tokens before hitting the limit.
    pub fn remaining_tokens(&self) -> i32 {
        self.current_model_limit as i32 - self.current_conversation_tokens as i32
    }

    /// Estimate token count for messages.
    /// This is a rough approximation: ~4 characters per token for English text.
    fn estimate_tokens(messages: &[Message], system_prompt: Option<&str>) -> u32 {
        let mut char_count = 0;

        // Count system prompt
        if let Some(prompt) = system_prompt {
            char_count += prompt.len();
        }

        // Count all message content
        for msg in messages {
            // Role overhead (roughly 10 tokens per message for formatting)
            char_count += 40; // ~10 tokens * 4 chars

            for block in &msg.content {
                match block {
                    ContentBlock::Text { text } => {
                        char_count += text.len();
                    }
                    ContentBlock::ToolUse { name, input, .. } => {
                        char_count += name.len();
                        char_count += input.to_string().len();
                        char_count += 100; // overhead for tool use structure
                    }
                    ContentBlock::ToolResult { content, .. } => {
                        char_count += content.len();
                        char_count += 50; // overhead for tool result structure
                    }
                }
            }
        }

        // Rough estimation: ~4 characters per token
        (char_count as u32 / 4).max(1)
    }

    /// Suggest how many messages to keep when compacting.
    /// Returns the index of the first message to keep.
    pub fn suggest_compaction_index(&self, messages: &[Message]) -> usize {
        if messages.len() < 10 {
            return 0; // Too few messages to compact
        }

        // Keep the most recent 20% of messages minimum
        let min_keep = messages.len() / 5;

        // Find a good cut point: after a user message, before assistant response
        let cutoff = messages.len().saturating_sub(min_keep);

        for i in (1..cutoff).rev() {
            if messages[i - 1].role == Role::User && messages[i].role == Role::Assistant {
                return i;
            }
        }

        // If no good cut point found, just use the cutoff
        cutoff
    }

    /// Get a status string for display in UI.
    pub fn status_string(&self) -> String {
        format!(
            "Context: {}/{} ({:.0}%)",
            self.current_conversation_tokens,
            self.current_model_limit,
            self.context_usage_percent()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_tracker_is_zero() {
        let tracker = ContextTracker::new();
        assert_eq!(tracker.total_input_tokens, 0);
        assert_eq!(tracker.total_output_tokens, 0);
        assert_eq!(tracker.total_tokens(), 0);
        assert_eq!(tracker.current_conversation_tokens, 0);
    }

    #[test]
    fn default_tracker_is_zero() {
        let tracker = ContextTracker::default();
        assert_eq!(tracker.total_tokens(), 0);
        assert_eq!(tracker.current_model_limit, 30_000);
    }

    #[test]
    fn record_single_usage() {
        let mut tracker = ContextTracker::new();
        tracker.record(&Usage {
            input_tokens: 100,
            output_tokens: 50,
        });
        assert_eq!(tracker.total_input_tokens, 100);
        assert_eq!(tracker.total_output_tokens, 50);
        assert_eq!(tracker.total_tokens(), 150);
    }

    #[test]
    fn record_multiple_usages_accumulates() {
        let mut tracker = ContextTracker::new();
        tracker.record(&Usage {
            input_tokens: 100,
            output_tokens: 50,
        });
        tracker.record(&Usage {
            input_tokens: 200,
            output_tokens: 75,
        });
        tracker.record(&Usage {
            input_tokens: 50,
            output_tokens: 25,
        });
        assert_eq!(tracker.total_input_tokens, 350);
        assert_eq!(tracker.total_output_tokens, 150);
        assert_eq!(tracker.total_tokens(), 500);
    }

    #[test]
    fn record_zero_usage() {
        let mut tracker = ContextTracker::new();
        tracker.record(&Usage {
            input_tokens: 0,
            output_tokens: 0,
        });
        assert_eq!(tracker.total_tokens(), 0);
    }

    #[test]
    fn set_model_exact_match() {
        let mut tracker = ContextTracker::new();
        tracker.set_model("claude-3-5-sonnet");
        assert_eq!(tracker.current_model_limit, 180_000);

        tracker.set_model("gpt-4");
        assert_eq!(tracker.current_model_limit, 7_500);

        tracker.set_model("gemini-1.5-pro");
        assert_eq!(tracker.current_model_limit, 1_000_000);
    }

    #[test]
    fn set_model_prefix_match() {
        let mut tracker = ContextTracker::new();
        tracker.set_model("claude-3-5-sonnet-20250101-preview");
        assert_eq!(tracker.current_model_limit, 180_000);

        tracker.set_model("gpt-4-0125-preview");
        assert_eq!(tracker.current_model_limit, 7_500);
    }

    #[test]
    fn set_model_unknown_uses_default() {
        let mut tracker = ContextTracker::new();
        tracker.set_model("unknown-model-xyz");
        assert_eq!(tracker.current_model_limit, 30_000);
    }

    #[test]
    fn estimate_tokens_simple_text() {
        let messages = vec![
            Message::user("Hello, world!"),
            Message::assistant("Hi there!"),
        ];

        let tokens = ContextTracker::estimate_tokens(&messages, None);
        // "Hello, world!" (13 chars) + "Hi there!" (9 chars) + overhead (~80 chars) = ~102 chars
        // ~102 chars / 4 = ~25 tokens
        assert!((20..=30).contains(&tokens));
    }

    #[test]
    fn estimate_tokens_with_system_prompt() {
        let messages = vec![Message::user("Hi")];
        let system_prompt = "You are a helpful assistant.";

        let tokens = ContextTracker::estimate_tokens(&messages, Some(system_prompt));
        // System prompt (29 chars) + "Hi" (2 chars) + overhead (~40 chars) = ~71 chars
        // ~71 chars / 4 = ~18 tokens
        assert!((15..=25).contains(&tokens));
    }

    #[test]
    fn estimate_tokens_with_tools() {
        let messages = vec![
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::text("I'll read that file."),
                    ContentBlock::tool_use(
                        "id-1",
                        "file_read",
                        serde_json::json!({"path": "/tmp/test.txt"}),
                    ),
                ],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::tool_result(
                    "id-1",
                    "File contents here",
                    false,
                )],
            },
        ];

        let tokens = ContextTracker::estimate_tokens(&messages, None);
        // Should account for text + tool overhead
        assert!(tokens >= 50);
    }

    #[test]
    fn context_usage_calculations() {
        let mut tracker = ContextTracker::new();
        tracker.current_model_limit = 1000;
        tracker.current_conversation_tokens = 250;

        assert_eq!(tracker.context_usage_percent(), 25.0);
        assert_eq!(tracker.remaining_tokens(), 750);
        assert!(!tracker.is_nearing_limit());

        tracker.current_conversation_tokens = 850;
        assert_eq!(tracker.context_usage_percent(), 85.0);
        assert_eq!(tracker.remaining_tokens(), 150);
        assert!(tracker.is_nearing_limit());
    }

    #[test]
    fn suggest_compaction_index_too_few_messages() {
        let tracker = ContextTracker::new();
        let messages = vec![Message::user("Hi"), Message::assistant("Hello")];

        assert_eq!(tracker.suggest_compaction_index(&messages), 0);
    }

    #[test]
    fn suggest_compaction_index_finds_cut_point() {
        let tracker = ContextTracker::new();
        let mut messages = Vec::new();

        // Create 20 messages alternating user/assistant
        for i in 0..10 {
            messages.push(Message::user(format!("Question {}", i)));
            messages.push(Message::assistant(format!("Answer {}", i)));
        }

        let index = tracker.suggest_compaction_index(&messages);
        // Should keep at least 20% (4 messages)
        assert!(index <= 16);
        // Should cut after a user message
        if index > 0 {
            assert_eq!(messages[index - 1].role, Role::User);
            assert_eq!(messages[index].role, Role::Assistant);
        }
    }
}
