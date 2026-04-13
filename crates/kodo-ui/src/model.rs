use crate::{
    keybinds::{KeyBindRegistry, LeaderState},
    slash::{SlashCommand, SlashState, builtin_commands},
    syntax::SyntaxHighlighter,
    theme::Theme,
};

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

    // -- Slash command state --
    /// Available slash commands, including discovered skills.
    pub commands: Vec<SlashCommand>,
    /// Active slash command autocomplete state
    pub slash_state: Option<SlashState>,
    /// Pending skill text to prepend to the next outbound message.
    pub pending_skill_injection: Option<String>,

    // -- Debug state --
    /// Whether debug mode is enabled (--debug flag)
    pub debug_mode: bool,

    // -- UI state --
    /// Active color theme
    pub theme: Theme,
    /// Syntax highlighter for code blocks (lazy-initialized)
    pub syntax_highlighter: Option<SyntaxHighlighter>,
    /// Keybind registry for customizable shortcuts
    pub keybinds: KeyBindRegistry,
    /// State for handling leader key sequences
    pub leader_state: LeaderState,

    // -- Application lifecycle --
    /// Whether the application should terminate
    pub should_quit: bool,
}

impl Model {
    /// Create a new Model with default values.
    /// Takes debug_mode as a parameter since it's set from CLI args.
    pub fn new(debug_mode: bool) -> Self {
        let keybinds = KeyBindRegistry::new();
        let leader_timeout = keybinds.leader_timeout_ms();
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

            // Slash state
            commands: builtin_commands(),
            slash_state: None,
            pending_skill_injection: None,

            // Debug state
            debug_mode,

            // UI state
            theme: Theme::dark(),
            syntax_highlighter: None, // Lazy-initialized
            keybinds,
            leader_state: LeaderState::new(leader_timeout),

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

    /// Check if slash mode is active.
    pub fn slash_is_active(&self) -> bool {
        self.slash_state.is_some()
    }

    /// Get or initialize the syntax highlighter.
    pub fn get_syntax_highlighter(&mut self) -> &SyntaxHighlighter {
        if self.syntax_highlighter.is_none() {
            let mut highlighter = SyntaxHighlighter::new();
            highlighter.set_theme(self.theme.is_dark());
            self.syntax_highlighter = Some(highlighter);
        }
        self.syntax_highlighter.as_ref().unwrap()
    }

    /// Update syntax highlighter theme when theme changes.
    pub fn update_syntax_theme(&mut self) {
        if let Some(ref mut highlighter) = self.syntax_highlighter {
            highlighter.set_theme(self.theme.is_dark());
        }
    }
}

impl Default for Model {
    fn default() -> Self {
        Self::new(false)
    }
}
