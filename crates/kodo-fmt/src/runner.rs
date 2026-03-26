use std::path::Path;

use anyhow::Result;
use tracing::{debug, warn};

use crate::registry::{FormatterConfig, FormatterRegistry};

/// Result of a formatting operation.
#[derive(Debug)]
pub struct FormatResult {
    /// The formatter that was used (None if no formatter matched).
    pub formatter_name: Option<String>,
    /// Whether the formatter ran successfully.
    pub success: bool,
    /// Human-readable message about what happened.
    pub message: String,
}

/// Run the appropriate formatter for a file, if one is configured.
///
/// Returns `None` if no formatter matches the file extension.
/// The formatter modifies the file in-place (silent formatting).
pub async fn format_file(registry: &FormatterRegistry, file_path: &Path) -> Option<FormatResult> {
    let config = registry.formatter_for(file_path)?;

    debug!(
        formatter = %config.name,
        file = %file_path.display(),
        "running formatter"
    );

    let result = run_formatter(config, file_path).await;

    match &result {
        Ok(msg) => {
            debug!(formatter = %config.name, "formatter succeeded");
            Some(FormatResult {
                formatter_name: Some(config.name.clone()),
                success: true,
                message: msg.clone(),
            })
        }
        Err(e) => {
            warn!(formatter = %config.name, error = %e, "formatter failed");
            Some(FormatResult {
                formatter_name: Some(config.name.clone()),
                success: false,
                message: format!("Formatter '{}' failed: {}", config.name, e),
            })
        }
    }
}

/// Execute a formatter command on a file.
async fn run_formatter(config: &FormatterConfig, file_path: &Path) -> Result<String> {
    let file_str = file_path.to_string_lossy().to_string();

    // Build the actual command, replacing $FILE with the file path.
    let args: Vec<String> = config
        .command
        .iter()
        .map(|arg| arg.replace("$FILE", &file_str))
        .collect();

    if args.is_empty() {
        anyhow::bail!("formatter command is empty");
    }

    let program = &args[0];
    let cmd_args = &args[1..];

    let output = tokio::process::Command::new(program)
        .args(cmd_args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await?;

    if output.status.success() {
        Ok(format!(
            "Formatted {} with {}",
            file_path.display(),
            config.name
        ))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "{} exited with code {}: {}",
            config.name,
            output.status.code().unwrap_or(-1),
            stderr.trim()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::FormatterConfig;
    use tempfile::TempDir;

    #[tokio::test]
    async fn format_no_matching_formatter() {
        let registry = FormatterRegistry::new(); // empty registry
        let result = format_file(&registry, Path::new("test.rs")).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn format_unknown_extension() {
        let registry = FormatterRegistry::with_builtins();
        let result = format_file(&registry, Path::new("test.xyz")).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn format_with_echo_formatter() {
        // Register a harmless formatter that just exits successfully
        let mut registry = FormatterRegistry::new();
        registry.register(FormatterConfig {
            name: "echo-fmt".into(),
            command: vec!["true".into()], // just exit 0
            extensions: vec!["txt".into()],
        });

        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "content").unwrap();

        let result = format_file(&registry, &file_path).await;
        assert!(result.is_some());
        let result = result.unwrap();
        assert!(result.success);
        assert_eq!(result.formatter_name, Some("echo-fmt".into()));
    }

    #[tokio::test]
    async fn format_with_failing_formatter() {
        let mut registry = FormatterRegistry::new();
        registry.register(FormatterConfig {
            name: "bad-fmt".into(),
            command: vec!["false".into()], // always exits 1
            extensions: vec!["txt".into()],
        });

        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "content").unwrap();

        let result = format_file(&registry, &file_path).await;
        assert!(result.is_some());
        let result = result.unwrap();
        assert!(!result.success);
        assert!(result.message.contains("failed"));
    }

    #[tokio::test]
    async fn format_replaces_file_placeholder() {
        // Use 'test -f $FILE' which checks if the file exists
        let mut registry = FormatterRegistry::new();
        registry.register(FormatterConfig {
            name: "file-check".into(),
            command: vec!["test".into(), "-f".into(), "$FILE".into()],
            extensions: vec!["txt".into()],
        });

        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "content").unwrap();

        let result = format_file(&registry, &file_path).await;
        assert!(result.is_some());
        assert!(result.unwrap().success);
    }
}
