/// Messages represent all possible state changes in the application.
/// Following the Elm Architecture pattern, Messages are the ONLY way
/// to modify the application state. They describe what happened, not
/// how to handle it (that's the job of update()).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    // -- Input events --
    /// User typed a character in the input field
    KeyInput(char),
    /// User pressed backspace
    Backspace,
    /// User pressed delete
    Delete,
    /// Move cursor left in input field
    CursorLeft,
    /// Move cursor right in input field
    CursorRight,
    /// Move cursor to beginning of input field
    CursorHome,
    /// Move cursor to end of input field
    CursorEnd,
    /// User pressed Enter to submit input
    Submit,
    /// Move slash completion selection
    SlashNav(i32),
    /// Execute current slash command
    SlashExecute,
    /// Cancel slash mode without clearing input
    SlashCancel,
    /// Scroll chat history up by given lines
    ScrollUp(u16),
    /// Scroll chat history down by given lines
    ScrollDown(u16),

    // -- Mode --
    /// Toggle between Plan/Build mode (Tab key)
    ToggleMode,

    // -- Theme --
    /// Change the active theme
    SetTheme(ThemeChoice),

    // -- Keybinds --
    /// Start waiting for leader key sequence
    StartLeaderSequence,
    /// Execute a leader key action
    ExecuteLeaderAction(crossterm::event::KeyCode),
    /// Cancel current leader sequence
    CancelLeaderSequence,

    // -- Agent lifecycle --
    /// Agent is streaming text tokens
    AgentTextDelta(String),
    /// Agent finished streaming response
    AgentTextDone,
    /// Agent started executing a tool
    AgentToolStart { name: String },
    /// Agent finished executing a tool
    AgentToolDone { name: String, success: bool },
    /// Agent tool execution was denied by permissions
    AgentToolDenied { name: String, reason: String },
    /// Agent tool execution was cancelled by user
    AgentToolCancelled { name: String },
    /// Agent formatted a file (post-edit)
    AgentFormatted { message: String },
    /// Agent collected LSP diagnostics after a file edit
    AgentDiagnostics { summary: String, count: usize },
    /// Agent encountered an error
    AgentError(String),
    /// Agent finished processing (no more streaming or tools)
    AgentDone,
    /// Runtime produced a user-visible informational message
    Notice(String),
    /// Runtime listed models available to the current provider/auth
    ModelsListed {
        current_model: String,
        models: Vec<String>,
    },
    /// Runtime listed providers from the auth store
    ProvidersListed(Vec<String>),
    /// Runtime switched to a new model
    ModelChanged(String),
    /// Runtime switched to a new provider and default model
    ProviderChanged { provider: String, model: String },
    /// Runtime completed a login request
    LoginComplete {
        account_id: String,
        name: Option<String>,
    },
    /// Runtime completed a logout request
    LogoutComplete(String),

    // -- System --
    /// Periodic tick for animations/updates
    Tick,
    /// Terminal was resized
    Resize(u16, u16),
    /// Request application shutdown
    Quit,
}

/// Theme choice for the SetTheme message
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeChoice {
    Dark,
    Light,
}
