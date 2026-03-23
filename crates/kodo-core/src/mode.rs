use std::fmt;

/// The operating mode of the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Read-only: search, read files, web fetch. No writes or shell execution.
    Plan,
    /// Full execution: all tools enabled, prompt on high-risk actions.
    Build,
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plan => write!(f, "plan"),
            Self::Build => write!(f, "build"),
        }
    }
}

impl Default for Mode {
    fn default() -> Self {
        Self::Build
    }
}
