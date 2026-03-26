use std::time::Duration;

use anyhow::Result;
use tracing::debug;

use crate::file_read::resolve_path;
use crate::tool::{PermissionLevel, Tool, ToolContext, ToolOutput};

const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const MAX_OUTPUT_BYTES: usize = 51_200;

/// Tool that searches file contents using ripgrep (`rg`).
///
/// Faster than the built-in `grep_search` for large codebases. Only
/// registered when `rg` is available on `PATH`.
pub struct RgSearchTool;

/// Check whether `rg` is available on PATH.
pub fn rg_available() -> bool {
    std::process::Command::new("rg")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

impl Tool for RgSearchTool {
    fn name(&self) -> &str {
        "rg_search"
    }

    fn description(&self) -> &str {
        "Search file contents using ripgrep (rg). Faster than grep_search for large \
         codebases. Supports regex patterns, file type filtering, and context lines. \
         Returns matching lines with file paths and line numbers."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The regex pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Directory or file to search in. Defaults to working directory."
                },
                "include": {
                    "type": "string",
                    "description": "Glob pattern to filter files (e.g. '*.rs', '*.{ts,tsx}')"
                },
                "fixed_strings": {
                    "type": "boolean",
                    "description": "Treat the pattern as a literal string, not a regex. Default false."
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": "Case-insensitive search. Default false."
                },
                "context_lines": {
                    "type": "integer",
                    "description": "Number of context lines to show around each match. Default 0."
                },
                "max_count": {
                    "type": "integer",
                    "description": "Maximum number of matches per file. No limit by default."
                }
            },
            "required": ["pattern"]
        })
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
    }

    fn execute(
        &self,
        params: serde_json::Value,
        ctx: &ToolContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ToolOutput>> + Send + '_>> {
        let working_dir = ctx.working_dir.clone();
        Box::pin(async move {
            let pattern = params
                .get("pattern")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("missing required parameter: pattern"))?;

            let search_path = params
                .get("path")
                .and_then(|v| v.as_str())
                .map(|p| resolve_path(&working_dir, p))
                .unwrap_or_else(|| working_dir.clone());

            let include = params.get("include").and_then(|v| v.as_str());
            let fixed_strings = params
                .get("fixed_strings")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let case_insensitive = params
                .get("case_insensitive")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let context_lines = params.get("context_lines").and_then(|v| v.as_u64());
            let max_count = params.get("max_count").and_then(|v| v.as_u64());

            debug!(
                pattern,
                path = %search_path.display(),
                fixed_strings,
                case_insensitive,
                "rg search"
            );

            let mut cmd = tokio::process::Command::new("rg");
            cmd.arg("--line-number")
                .arg("--no-heading")
                .arg("--color=never");

            if fixed_strings {
                cmd.arg("--fixed-strings");
            }

            if case_insensitive {
                cmd.arg("--ignore-case");
            }

            if let Some(ctx_lines) = context_lines {
                cmd.arg(format!("--context={ctx_lines}"));
            }

            if let Some(max) = max_count {
                cmd.arg(format!("--max-count={max}"));
            }

            if let Some(glob) = include {
                cmd.arg("--glob").arg(glob);
            }

            cmd.arg("--").arg(pattern).arg(&search_path);

            cmd.stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());

            let child = cmd.spawn()?;

            let output = match tokio::time::timeout(
                Duration::from_millis(DEFAULT_TIMEOUT_MS),
                child.wait_with_output(),
            )
            .await
            {
                Ok(Ok(output)) => output,
                Ok(Err(e)) => {
                    return Ok(ToolOutput {
                        content: format!("rg failed to execute: {e}"),
                        success: false,
                    });
                }
                Err(_) => {
                    return Ok(ToolOutput {
                        content: format!("rg timed out after {DEFAULT_TIMEOUT_MS}ms"),
                        success: false,
                    });
                }
            };

            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            // rg exits 1 when no matches are found (not an error).
            if output.status.code() == Some(1) && stderr.is_empty() {
                return Ok(ToolOutput {
                    content: format!("No matches found for pattern: {pattern}"),
                    success: true,
                });
            }

            // rg exits 2+ for actual errors.
            if !output.status.success() && output.status.code() != Some(1) {
                return Ok(ToolOutput {
                    content: format!(
                        "rg error (exit {}): {}",
                        output.status.code().unwrap_or(-1),
                        stderr.trim()
                    ),
                    success: false,
                });
            }

            // Make paths relative to working dir for cleaner output.
            let result = if search_path == working_dir {
                stdout.to_string()
            } else {
                let prefix = working_dir.to_string_lossy();
                stdout.replace(&format!("{}/", prefix), "")
            };

            let truncated = result.len() > MAX_OUTPUT_BYTES;
            let display = if truncated {
                format!(
                    "{}\n\n... (output truncated at {} bytes, total {} bytes)",
                    &result[..MAX_OUTPUT_BYTES],
                    MAX_OUTPUT_BYTES,
                    result.len()
                )
            } else {
                result
            };

            Ok(ToolOutput {
                content: display,
                success: true,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext {
            working_dir: dir.to_path_buf(),
        }
    }

    fn skip_if_no_rg() -> bool {
        !rg_available()
    }

    #[test]
    fn rg_available_returns_bool() {
        // Just ensure it doesn't panic. Result depends on environment.
        let _ = rg_available();
    }

    #[tokio::test]
    async fn rg_search_basic() {
        if skip_if_no_rg() {
            return;
        }

        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("test.txt"),
            "hello world\nfoo bar\nhello again\n",
        )
        .unwrap();

        let tool = RgSearchTool;
        let ctx = make_ctx(dir.path());
        let params = serde_json::json!({"pattern": "hello"});
        let result = tool.execute(params, &ctx).await.unwrap();
        assert!(result.success);
        assert!(result.content.contains("hello world"));
        assert!(result.content.contains("hello again"));
    }

    #[tokio::test]
    async fn rg_search_no_matches() {
        if skip_if_no_rg() {
            return;
        }

        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello world\n").unwrap();

        let tool = RgSearchTool;
        let ctx = make_ctx(dir.path());
        let params = serde_json::json!({"pattern": "nonexistent"});
        let result = tool.execute(params, &ctx).await.unwrap();
        assert!(result.success);
        assert!(result.content.contains("No matches found"));
    }

    #[tokio::test]
    async fn rg_search_with_glob_filter() {
        if skip_if_no_rg() {
            return;
        }

        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("code.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.path().join("readme.md"), "fn not_code\n").unwrap();

        let tool = RgSearchTool;
        let ctx = make_ctx(dir.path());
        let params = serde_json::json!({"pattern": "fn", "include": "*.rs"});
        let result = tool.execute(params, &ctx).await.unwrap();
        assert!(result.success);
        assert!(result.content.contains("code.rs"));
        assert!(!result.content.contains("readme.md"));
    }

    #[tokio::test]
    async fn rg_search_fixed_strings() {
        if skip_if_no_rg() {
            return;
        }

        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("test.txt"), "a.b\na*b\na+b\n").unwrap();

        let tool = RgSearchTool;
        let ctx = make_ctx(dir.path());
        // Without fixed_strings, "a.b" would match "a*b" too (dot = any char)
        let params = serde_json::json!({"pattern": "a.b", "fixed_strings": true});
        let result = tool.execute(params, &ctx).await.unwrap();
        assert!(result.success);
        assert!(result.content.contains("a.b"));
        assert!(!result.content.contains("a*b"));
    }

    #[tokio::test]
    async fn rg_search_case_insensitive() {
        if skip_if_no_rg() {
            return;
        }

        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("test.txt"), "Hello\nhello\nHELLO\n").unwrap();

        let tool = RgSearchTool;
        let ctx = make_ctx(dir.path());
        let params = serde_json::json!({"pattern": "hello", "case_insensitive": true});
        let result = tool.execute(params, &ctx).await.unwrap();
        assert!(result.success);
        assert!(result.content.contains("Hello"));
        assert!(result.content.contains("HELLO"));
    }

    #[tokio::test]
    async fn rg_search_missing_pattern() {
        if skip_if_no_rg() {
            return;
        }

        let tool = RgSearchTool;
        let ctx = make_ctx(std::path::Path::new("/tmp"));
        let params = serde_json::json!({});
        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
    }
}
