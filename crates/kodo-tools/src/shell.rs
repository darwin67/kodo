use std::time::Duration;

use anyhow::Result;
use tracing::debug;

use crate::tool::{PermissionLevel, Tool, ToolContext, ToolOutput};

const DEFAULT_TIMEOUT_MS: u64 = 120_000; // 2 minutes
const MAX_OUTPUT_BYTES: usize = 51_200;

/// Tool that executes shell commands.
pub struct ShellTool;

impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Execute a shell command and return stdout and stderr. Commands run in the \
         working directory. Use this for running builds, tests, git operations, etc."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Timeout in milliseconds. Default 120000 (2 minutes)."
                }
            },
            "required": ["command"]
        })
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Execute
    }

    fn execute(
        &self,
        params: serde_json::Value,
        ctx: &ToolContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ToolOutput>> + Send + '_>> {
        let working_dir = ctx.working_dir.clone();
        Box::pin(async move {
            let command = params
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("missing required parameter: command"))?;

            let timeout_ms = params
                .get("timeout_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(DEFAULT_TIMEOUT_MS);

            debug!(command, timeout_ms, dir = %working_dir.display(), "executing shell command");

            let child = tokio::process::Command::new("sh")
                .arg("-c")
                .arg(command)
                .current_dir(&working_dir)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()?;

            let result =
                tokio::time::timeout(Duration::from_millis(timeout_ms), child.wait_with_output())
                    .await;

            let output = match result {
                Ok(Ok(output)) => output,
                Ok(Err(e)) => {
                    return Ok(ToolOutput {
                        content: format!("Command failed to execute: {e}"),
                        success: false,
                    });
                }
                Err(_) => {
                    return Ok(ToolOutput {
                        content: format!("Command timed out after {}ms", timeout_ms),
                        success: false,
                    });
                }
            };

            let exit_code = output.status.code().unwrap_or(-1);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            let mut content = String::new();
            content.push_str(&format!("Exit code: {exit_code}\n"));

            if !stdout.is_empty() {
                let stdout_str = truncate_output(&stdout, MAX_OUTPUT_BYTES);
                content.push_str(&format!("\nSTDOUT:\n{stdout_str}"));
            }

            if !stderr.is_empty() {
                let stderr_str = truncate_output(&stderr, MAX_OUTPUT_BYTES);
                content.push_str(&format!("\nSTDERR:\n{stderr_str}"));
            }

            Ok(ToolOutput {
                content,
                success: output.status.success(),
            })
        })
    }
}

fn truncate_output(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        s.to_string()
    } else {
        let truncated = &s[..max_bytes];
        format!(
            "{truncated}\n\n... (output truncated at {max_bytes} bytes, total {} bytes)",
            s.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ctx() -> ToolContext {
        ToolContext {
            working_dir: std::env::current_dir().unwrap(),
        }
    }

    #[tokio::test]
    async fn shell_echo() {
        let tool = ShellTool;
        let ctx = make_ctx();
        let params = serde_json::json!({"command": "echo hello"});
        let result = tool.execute(params, &ctx).await.unwrap();
        assert!(result.success);
        assert!(result.content.contains("Exit code: 0"));
        assert!(result.content.contains("hello"));
    }

    #[tokio::test]
    async fn shell_failing_command() {
        let tool = ShellTool;
        let ctx = make_ctx();
        let params = serde_json::json!({"command": "false"});
        let result = tool.execute(params, &ctx).await.unwrap();
        assert!(!result.success);
        assert!(result.content.contains("Exit code: 1"));
    }

    #[tokio::test]
    async fn shell_stderr() {
        let tool = ShellTool;
        let ctx = make_ctx();
        let params = serde_json::json!({"command": "echo error >&2"});
        let result = tool.execute(params, &ctx).await.unwrap();
        assert!(result.success);
        assert!(result.content.contains("STDERR"));
        assert!(result.content.contains("error"));
    }

    #[tokio::test]
    async fn shell_timeout() {
        let tool = ShellTool;
        let ctx = make_ctx();
        let params = serde_json::json!({"command": "sleep 10", "timeout_ms": 100});
        let result = tool.execute(params, &ctx).await.unwrap();
        assert!(!result.success);
        assert!(result.content.contains("timed out"));
    }

    #[tokio::test]
    async fn shell_working_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = ToolContext {
            working_dir: dir.path().to_path_buf(),
        };
        let tool = ShellTool;
        let params = serde_json::json!({"command": "pwd"});
        let result = tool.execute(params, &ctx).await.unwrap();
        assert!(result.success);
        // macOS may have /private prefix for temp dirs
        let dir_str = dir.path().to_str().unwrap();
        let content_normalized = result.content.replace("/private", "");
        assert!(
            content_normalized.contains(dir_str),
            "expected '{}' in output: {}",
            dir_str,
            result.content
        );
    }

    #[tokio::test]
    async fn shell_missing_command() {
        let tool = ShellTool;
        let ctx = make_ctx();
        let params = serde_json::json!({});
        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
    }

    #[test]
    fn truncate_short_output() {
        let s = "short";
        assert_eq!(truncate_output(s, 100), "short");
    }

    #[test]
    fn truncate_long_output() {
        let s = "a".repeat(200);
        let result = truncate_output(&s, 50);
        assert!(result.contains("truncated"));
        assert!(result.contains("50 bytes"));
    }
}
