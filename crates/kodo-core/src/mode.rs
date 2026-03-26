use std::fmt;

use kodo_tools::tool::PermissionLevel;

/// The operating mode of the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Read-only: search, read files, web fetch. No writes or shell execution.
    Plan,
    /// Full execution: all tools enabled, prompt on high-risk actions.
    #[default]
    Build,
}

impl Mode {
    /// Whether a tool with the given permission level is allowed in this mode.
    pub fn allows(&self, level: PermissionLevel) -> bool {
        match self {
            Self::Plan => matches!(level, PermissionLevel::Read),
            Self::Build => true,
        }
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plan => write!(f, "plan"),
            Self::Build => write!(f, "build"),
        }
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

    // ----- Permission filtering tests -----

    #[test]
    fn plan_allows_read() {
        assert!(Mode::Plan.allows(PermissionLevel::Read));
    }

    #[test]
    fn plan_denies_write() {
        assert!(!Mode::Plan.allows(PermissionLevel::Write));
    }

    #[test]
    fn plan_denies_execute() {
        assert!(!Mode::Plan.allows(PermissionLevel::Execute));
    }

    #[test]
    fn build_allows_read() {
        assert!(Mode::Build.allows(PermissionLevel::Read));
    }

    #[test]
    fn build_allows_write() {
        assert!(Mode::Build.allows(PermissionLevel::Write));
    }

    #[test]
    fn build_allows_execute() {
        assert!(Mode::Build.allows(PermissionLevel::Execute));
    }
}
