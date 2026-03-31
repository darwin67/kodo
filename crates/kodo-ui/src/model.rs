use crate::theme::Theme;

/// A message in the conversation display.
/// Represents both user inputs and agent responses.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

/// The role of a chat message participant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Assistant,
    Tool,
    System,
}

/// The complete application state following the Elm Architecture.
/// This struct contains ALL mutable state in the application.
/// No methods should perform side effects - this is pure data.
/// All state changes happen through the update() function.
#[derive(Debug)]
pub struct Model {
    // -- Input state --
    /// Current user input buffer
    pub input: String,
    /// Cursor position within the input field
    pub cursor_pos: usize,

    // -- Chat state --
    /// Complete conversation history
    pub messages: Vec<ChatMessage>,
    /// Scroll offset for the output panel (0 = show latest)
    pub scroll_offset: u16,
    /// Whether the agent is currently streaming a response
    pub is_streaming: bool,
    /// Current streaming text buffer (not yet added to messages)
    pub streaming_text: String,

    // -- Session info --
    /// Current mode display string (Plan/Build)
    pub mode: String,
    /// Current LLM provider name (anthropic, openai, etc.)
    pub provider: String,
    /// Current model name (claude-3-5-sonnet, gpt-4, etc.)
    pub model_name: String,
    /// Total input tokens consumed this session
    pub input_tokens: u64,
    /// Total output tokens generated this session
    pub output_tokens: u64,

    // -- Command palette state --
    /// Whether the command palette is currently open
    pub palette_open: bool,
    /// User's current search query in the palette
    pub palette_query: String,
    /// Index of the currently selected palette item
    pub palette_selected: usize,

    // -- Debug state --
    /// Whether debug mode is enabled (--debug flag)
    pub debug_mode: bool,
    /// Whether the debug panel is currently visible
    pub debug_panel_open: bool,
    /// Debug log entries
    pub debug_logs: Vec<String>,
    /// Scroll offset for the debug panel
    pub debug_scroll: u16,

    // -- UI state --
    /// Active color theme
    pub theme: Theme,

    // -- Application lifecycle --
    /// Whether the application should terminate
    pub should_quit: bool,
}

impl Model {
    /// Create a new Model with default values.
    /// Takes debug_mode as a parameter since it's set from CLI args.
    pub fn new(debug_mode: bool) -> Self {
        Self {
            // Input state
            input: String::new(),
            cursor_pos: 0,

            // Chat state
            messages: Vec::new(),
            scroll_offset: 0,
            is_streaming: false,
            streaming_text: String::new(),

            // Session info - will be updated when agent starts
            mode: "Build".to_string(),
            provider: "unknown".to_string(),
            model_name: "unknown".to_string(),
            input_tokens: 0,
            output_tokens: 0,

            // Palette state
            palette_open: false,
            palette_query: String::new(),
            palette_selected: 0,

            // Debug state
            debug_mode,
            debug_panel_open: false,
            debug_logs: Vec::new(),
            debug_scroll: 0,

            // UI state
            theme: Theme::dark(),

            // Application lifecycle
            should_quit: false,
        }
    }

    /// Get the current input as a string slice.
    pub fn input(&self) -> &str {
        &self.input
    }

    /// Get the current streaming text.
    pub fn streaming_text(&self) -> &str {
        &self.streaming_text
    }

    /// Check if the input field is empty.
    pub fn input_is_empty(&self) -> bool {
        self.input.is_empty()
    }

    /// Check if we're currently streaming a response.
    pub fn is_streaming(&self) -> bool {
        self.is_streaming
    }

    /// Get the total number of messages in the conversation.
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Check if the command palette is open.
    pub fn palette_is_open(&self) -> bool {
        self.palette_open
    }

    /// Check if the debug panel is open.
    pub fn debug_panel_is_open(&self) -> bool {
        self.debug_panel_open
    }
}

impl Default for Model {
    fn default() -> Self {
        Self::new(false)
    }
}
