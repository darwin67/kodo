use kodo_llm::types::Usage;

/// Tracks token usage across a session.
///
/// In Phase 1, this is a simple accumulator. Later phases will add
/// compaction, summarization, and context window management.
#[derive(Debug, Default)]
pub struct ContextTracker {
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
}

impl ContextTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record token usage from a completion.
    pub fn record(&mut self, usage: &Usage) {
        self.total_input_tokens += usage.input_tokens as u64;
        self.total_output_tokens += usage.output_tokens as u64;
    }

    /// Total tokens consumed in this session.
    pub fn total_tokens(&self) -> u64 {
        self.total_input_tokens + self.total_output_tokens
    }
}
