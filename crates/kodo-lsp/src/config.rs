/// Configuration for an LSP server.
#[derive(Debug, Clone)]
pub struct LspServerConfig {
    /// Display name.
    pub name: &'static str,
    /// Command to start the server.
    pub command: &'static str,
    /// Arguments to the command.
    pub args: &'static [&'static str],
    /// File extensions this server handles (without dot).
    pub extensions: &'static [&'static str],
}

/// Built-in LSP server configurations.
pub fn builtin_configs() -> Vec<LspServerConfig> {
    vec![
        LspServerConfig {
            name: "rust-analyzer",
            command: "rust-analyzer",
            args: &[],
            extensions: &["rs"],
        },
        LspServerConfig {
            name: "gopls",
            command: "gopls",
            args: &["serve"],
            extensions: &["go"],
        },
        LspServerConfig {
            name: "typescript-language-server",
            command: "typescript-language-server",
            args: &["--stdio"],
            extensions: &["ts", "tsx", "js", "jsx"],
        },
        LspServerConfig {
            name: "pyright",
            command: "pyright-langserver",
            args: &["--stdio"],
            extensions: &["py", "pyi"],
        },
    ]
}

/// Check if a command is available on PATH.
pub fn is_command_available(command: &str) -> bool {
    std::process::Command::new("which")
        .arg(command)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_configs_not_empty() {
        let configs = builtin_configs();
        assert!(!configs.is_empty());
    }

    #[test]
    fn rust_analyzer_config_has_rs_extension() {
        let configs = builtin_configs();
        let ra = configs.iter().find(|c| c.name == "rust-analyzer").unwrap();
        assert!(ra.extensions.contains(&"rs"));
    }

    #[test]
    fn gopls_config_has_go_extension() {
        let configs = builtin_configs();
        let gopls = configs.iter().find(|c| c.name == "gopls").unwrap();
        assert!(gopls.extensions.contains(&"go"));
    }

    #[test]
    fn typescript_config_has_all_extensions() {
        let configs = builtin_configs();
        let ts = configs
            .iter()
            .find(|c| c.name == "typescript-language-server")
            .unwrap();
        assert!(ts.extensions.contains(&"ts"));
        assert!(ts.extensions.contains(&"tsx"));
        assert!(ts.extensions.contains(&"js"));
        assert!(ts.extensions.contains(&"jsx"));
    }

    #[test]
    fn is_command_available_finds_sh() {
        assert!(is_command_available("sh"));
    }

    #[test]
    fn is_command_available_returns_false_for_missing() {
        assert!(!is_command_available("kodo_nonexistent_command_xyz"));
    }
}
