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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_tracker_is_zero() {
        let tracker = ContextTracker::new();
        assert_eq!(tracker.total_input_tokens, 0);
        assert_eq!(tracker.total_output_tokens, 0);
        assert_eq!(tracker.total_tokens(), 0);
    }

    #[test]
    fn default_tracker_is_zero() {
        let tracker = ContextTracker::default();
        assert_eq!(tracker.total_tokens(), 0);
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
}
