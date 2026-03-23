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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mode_is_build() {
        assert_eq!(Mode::default(), Mode::Build);
    }

    #[test]
    fn mode_display_plan() {
        assert_eq!(Mode::Plan.to_string(), "plan");
    }

    #[test]
    fn mode_display_build() {
        assert_eq!(Mode::Build.to_string(), "build");
    }

    #[test]
    fn mode_equality() {
        assert_eq!(Mode::Plan, Mode::Plan);
        assert_eq!(Mode::Build, Mode::Build);
        assert_ne!(Mode::Plan, Mode::Build);
    }

    #[test]
    fn mode_clone() {
        let mode = Mode::Plan;
        let cloned = mode;
        assert_eq!(mode, cloned);
    }

    #[test]
    fn mode_debug() {
        assert_eq!(format!("{:?}", Mode::Plan), "Plan");
        assert_eq!(format!("{:?}", Mode::Build), "Build");
    }
}
