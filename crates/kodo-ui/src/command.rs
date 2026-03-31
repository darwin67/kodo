/// Commands represent side effects that the application needs to perform.
/// Following the Elm Architecture, Commands are returned from update()
/// and executed by the runtime loop. They describe WHAT to do, not HOW.
///
/// The update() function is pure - it only modifies the model and returns
/// Commands. The runtime loop executes Commands and feeds results back
/// as new Messages. This separation makes the core logic testable and
/// allows the same update logic to work with different runtimes (TUI, GUI).
#[derive(Debug, Clone)]
pub enum Command {
    /// Send a user message to the agent for processing.
    /// The runtime will send this over the agent channel and listen for responses.
    SendToAgent(String),

    /// Request the application to exit gracefully.
    /// The runtime will perform cleanup and terminate the event loop.
    Quit,

    /// No operation - a convenience variant for update() arms that don't
    /// need to perform side effects. Allows cleaner match expressions.
    None,
}

impl Command {
    /// Convenience constructor for SendToAgent
    pub fn send_to_agent(message: impl Into<String>) -> Self {
        Self::SendToAgent(message.into())
    }

    /// Check if this command is a no-op
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }
}
