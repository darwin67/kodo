use crate::{
    keybinds::{KeyBindRegistry, LeaderState},
    syntax::SyntaxHighlighter,
    theme::Theme,
};

/// Represents a connectable provider in the provider selection modal.
#[derive(Debug, Clone)]
pub struct ProviderOption {
    pub id: String,
    pub display_name: String,
    pub auth_methods: Vec<AuthMethod>,
    pub is_authenticated: bool,
}

/// How a provider can be authenticated.
#[derive(Debug, Clone, PartialEq)]
pub enum AuthMethod {
    /// OAuth auto-redirect flow (localhost callback server)
    OAuth,
    /// OAuth code-paste flow (user pastes code from browser)
    OAuthCodePaste,
    /// Manual API key entry
    ApiKey,
}

/// Represents a selectable model.
#[derive(Debug, Clone)]
pub struct ModelOption {
    pub id: String,
    pub display_name: String,
    pub provider: String,
}

/// State of the provider connect modal.
#[derive(Debug, Clone, PartialEq)]
pub enum ProviderModalState {
    /// Closed / not visible
    Closed,
    /// Showing the list of providers to connect
    SelectProvider,
    /// Showing auth method choice for a provider (OAuth vs API key)
    SelectAuthMethod { provider: String },
    /// Waiting for API key input
    EnterApiKey { provider: String },
    /// OAuth auto-redirect flow in progress (browser opened, waiting for callback)
    OAuthInProgress { provider: String },
    /// OAuth code-paste flow: browser opened, waiting for user to paste the code
    EnterOAuthCode { provider: String, auth_url: String },
    /// Auth succeeded, ready to pick model
    AuthSuccess { provider: String },
    /// Auth failed with an error message
    AuthError { provider: String, error: String },
}

/// State of the model selection modal.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelModalState {
    /// Closed / not visible
    Closed,
    /// Showing available models
    SelectModel,
}

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
    /// Current conversation token count
    pub context_tokens: u32,
    /// Model context window limit
    pub context_limit: u32,

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
    /// Syntax highlighter for code blocks (lazy-initialized)
    pub syntax_highlighter: Option<SyntaxHighlighter>,
    /// Keybind registry for customizable shortcuts
    pub keybinds: KeyBindRegistry,
    /// State for handling leader key sequences
    pub leader_state: LeaderState,

    // -- Provider connect modal --
    /// Current state of the provider connect modal
    pub provider_modal: ProviderModalState,
    /// Available providers for connection
    pub provider_options: Vec<ProviderOption>,
    /// Selected index in provider list
    pub provider_modal_selected: usize,
    /// Selected index in auth method list
    pub auth_method_selected: usize,
    /// API key input buffer (for manual entry)
    pub api_key_input: String,
    /// Whether the app launched without any provider (needs auth before use)
    pub needs_provider: bool,

    // -- Model selection modal --
    /// Current state of the model selection modal
    pub model_modal: ModelModalState,
    /// Available models for selection
    pub model_options: Vec<ModelOption>,
    /// Selected index in model list
    pub model_modal_selected: usize,

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
            context_tokens: 0,
            context_limit: 0,

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
            syntax_highlighter: None, // Lazy-initialized
            keybinds,
            leader_state: LeaderState::new(leader_timeout),

            // Provider connect modal
            provider_modal: ProviderModalState::Closed,
            provider_options: Vec::new(),
            provider_modal_selected: 0,
            auth_method_selected: 0,
            api_key_input: String::new(),
            needs_provider: false,

            // Model selection modal
            model_modal: ModelModalState::Closed,
            model_options: Vec::new(),
            model_modal_selected: 0,

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
