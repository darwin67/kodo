use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Main configuration structure for Kodo
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct Config {
    /// General settings
    #[serde(default)]
    pub general: GeneralConfig,

    /// Provider-specific settings
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,

    /// LSP server configurations
    #[serde(default)]
    pub lsp_servers: HashMap<String, LspConfig>,

    /// Formatter configurations
    #[serde(default)]
    pub formatters: HashMap<String, FormatterConfig>,

    /// Tool-specific settings
    #[serde(default)]
    pub tools: ToolsConfig,

    /// UI/TUI settings
    #[serde(default)]
    pub ui: UiConfig,
}

/// General application settings
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct GeneralConfig {
    /// Default provider to use
    #[serde(default = "default_provider")]
    pub default_provider: String,

    /// Default model to use
    #[serde(default = "default_model")]
    pub default_model: String,

    /// Default mode (Plan/Build)
    #[serde(default = "default_mode")]
    pub default_mode: String,

    /// Maximum concurrent subagents
    #[serde(default = "default_max_subagents")]
    pub max_subagents: usize,

    /// Enable debug logging
    #[serde(default)]
    pub debug: bool,

    /// Session storage directory
    #[serde(default)]
    pub session_dir: Option<PathBuf>,

    /// Automatically install missing LSP servers
    #[serde(default)]
    pub auto_install_lsp: bool,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            default_provider: default_provider(),
            default_model: default_model(),
            default_mode: default_mode(),
            max_subagents: default_max_subagents(),
            debug: false,
            session_dir: None,
            auto_install_lsp: false,
        }
    }
}

/// Provider-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ProviderConfig {
    /// API key (can also come from environment)
    pub api_key: Option<String>,

    /// Base URL override
    pub base_url: Option<String>,

    /// Default model for this provider
    pub default_model: Option<String>,

    /// Timeout in seconds
    pub timeout: Option<u64>,

    /// Enable prompt caching (Anthropic)
    #[serde(default = "default_true")]
    pub enable_caching: bool,

    /// Custom headers
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

/// LSP server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LspConfig {
    /// Command to start the server
    pub command: String,

    /// Command arguments
    #[serde(default)]
    pub args: Vec<String>,

    /// File extensions this server handles
    pub extensions: Vec<String>,

    /// Environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Working directory
    pub work_dir: Option<PathBuf>,

    /// Initialization options
    #[serde(default)]
    pub init_options: serde_json::Value,

    /// Auto-install command (if server not found)
    pub install_command: Option<String>,
}

/// Formatter configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct FormatterConfig {
    /// Command to run the formatter
    pub command: String,

    /// Command arguments (use {file} as placeholder)
    #[serde(default)]
    pub args: Vec<String>,

    /// File extensions this formatter handles
    pub extensions: Vec<String>,

    /// Environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Format on save
    #[serde(default = "default_true")]
    pub format_on_save: bool,

    /// Timeout in seconds
    #[serde(default = "default_format_timeout")]
    pub timeout: u64,
}

/// Tool-specific settings
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct ToolsConfig {
    /// Shell command settings
    #[serde(default)]
    pub shell: ShellConfig,

    /// File operation settings
    #[serde(default)]
    pub file_ops: FileOpsConfig,

    /// Web fetch settings
    #[serde(default)]
    pub web_fetch: WebFetchConfig,

    /// Disabled tools
    #[serde(default)]
    pub disabled: Vec<String>,
}

/// Shell tool configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ShellConfig {
    /// Shell to use (bash, zsh, etc.)
    #[serde(default = "default_shell")]
    pub shell: String,

    /// Shell arguments
    #[serde(default = "default_shell_args")]
    pub shell_args: Vec<String>,

    /// Working directory
    pub work_dir: Option<PathBuf>,

    /// Environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Command timeout in seconds
    #[serde(default = "default_shell_timeout")]
    pub timeout: u64,

    /// Dangerous command patterns (require confirmation)
    #[serde(default = "default_dangerous_patterns")]
    pub dangerous_patterns: Vec<String>,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            shell: default_shell(),
            shell_args: default_shell_args(),
            work_dir: None,
            env: HashMap::new(),
            timeout: default_shell_timeout(),
            dangerous_patterns: default_dangerous_patterns(),
        }
    }
}

/// File operations configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct FileOpsConfig {
    /// Maximum file size to read (in bytes)
    #[serde(default = "default_max_file_size")]
    pub max_file_size: u64,

    /// Excluded directories
    #[serde(default = "default_excluded_dirs")]
    pub excluded_dirs: Vec<String>,

    /// Excluded file patterns
    #[serde(default)]
    pub excluded_patterns: Vec<String>,

    /// Create backups before editing
    #[serde(default)]
    pub create_backups: bool,
}

/// Web fetch configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct WebFetchConfig {
    /// User agent string
    #[serde(default = "default_user_agent")]
    pub user_agent: String,

    /// Timeout in seconds
    #[serde(default = "default_web_timeout")]
    pub timeout: u64,

    /// Maximum response size (in bytes)
    #[serde(default = "default_max_response_size")]
    pub max_response_size: u64,

    /// Allowed domains (empty = all allowed)
    #[serde(default)]
    pub allowed_domains: Vec<String>,

    /// Custom headers
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

impl Default for WebFetchConfig {
    fn default() -> Self {
        Self {
            user_agent: default_user_agent(),
            timeout: default_web_timeout(),
            max_response_size: default_max_response_size(),
            allowed_domains: vec![],
            headers: HashMap::new(),
        }
    }
}

/// UI/TUI configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct UiConfig {
    /// Color theme
    #[serde(default = "default_theme")]
    pub theme: String,

    /// Enable mouse support
    #[serde(default = "default_true")]
    pub mouse: bool,

    /// Show status bar
    #[serde(default = "default_true")]
    pub status_bar: bool,

    /// Show line numbers in code blocks
    #[serde(default = "default_true")]
    pub line_numbers: bool,

    /// Keybindings
    #[serde(default)]
    pub keybinds: HashMap<String, String>,

    /// Leader key
    #[serde(default = "default_leader_key")]
    pub leader_key: String,

    /// Leader timeout in ms
    #[serde(default = "default_leader_timeout")]
    pub leader_timeout: u64,
}

// Default value functions
fn default_provider() -> String {
    "anthropic".to_string()
}

fn default_model() -> String {
    "claude-sonnet-4-20250514".to_string()
}

fn default_mode() -> String {
    "build".to_string()
}

fn default_max_subagents() -> usize {
    5
}

fn default_true() -> bool {
    true
}

fn default_format_timeout() -> u64 {
    30
}

fn default_shell() -> String {
    if cfg!(windows) {
        "cmd".to_string()
    } else {
        "bash".to_string()
    }
}

fn default_shell_args() -> Vec<String> {
    if cfg!(windows) {
        vec!["/C".to_string()]
    } else {
        vec!["-c".to_string()]
    }
}

fn default_shell_timeout() -> u64 {
    300 // 5 minutes
}

fn default_dangerous_patterns() -> Vec<String> {
    vec![
        "rm -rf".to_string(),
        "sudo".to_string(),
        "chmod 777".to_string(),
        "git push --force".to_string(),
        ":(){:|:&};:".to_string(), // fork bomb
    ]
}

fn default_max_file_size() -> u64 {
    10 * 1024 * 1024 // 10MB
}

fn default_excluded_dirs() -> Vec<String> {
    vec![
        ".git".to_string(),
        "node_modules".to_string(),
        "target".to_string(),
        ".venv".to_string(),
        "__pycache__".to_string(),
    ]
}

fn default_user_agent() -> String {
    format!("Kodo/{}", env!("CARGO_PKG_VERSION"))
}

fn default_web_timeout() -> u64 {
    30
}

fn default_max_response_size() -> u64 {
    50 * 1024 * 1024 // 50MB
}

fn default_theme() -> String {
    "dark".to_string()
}

fn default_leader_key() -> String {
    "ctrl-k".to_string()
}

fn default_leader_timeout() -> u64 {
    2000
}
