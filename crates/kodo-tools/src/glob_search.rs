use anyhow::Result;
use tracing::debug;

use crate::tool::{PermissionLevel, Tool, ToolContext, ToolOutput};

const MAX_RESULTS: usize = 200;

/// Tool that finds files matching a glob pattern.
pub struct GlobSearchTool;

impl Tool for GlobSearchTool {
    fn name(&self) -> &str {
        "glob_search"
    }

    fn description(&self) -> &str {
        "Find files matching a glob pattern (e.g. \"**/*.rs\", \"src/**/*.ts\"). \
         Returns matching file paths relative to the working directory."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The glob pattern to match files against (e.g. '**/*.rs')"
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in. Defaults to working directory."
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
            let pattern_str = params
                .get("pattern")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("missing required parameter: pattern"))?;

            let base_dir = params
                .get("path")
                .and_then(|v| v.as_str())
                .map(|p| {
                    let path = std::path::PathBuf::from(p);
                    if path.is_absolute() {
                        path
                    } else {
                        working_dir.join(path)
                    }
                })
                .unwrap_or_else(|| working_dir.clone());

            let full_pattern = base_dir.join(pattern_str);
            let full_pattern_str = full_pattern.to_string_lossy().to_string();

            debug!(pattern = %full_pattern_str, "glob search");

            let mut matches: Vec<String> = Vec::new();
            let mut truncated = false;

            for entry in glob::glob(&full_pattern_str)? {
                match entry {
                    Ok(path) => {
                        if matches.len() >= MAX_RESULTS {
                            truncated = true;
                            break;
                        }
                        // Show relative paths when possible.
                        let display_path = path
                            .strip_prefix(&working_dir)
                            .unwrap_or(&path)
                            .to_string_lossy()
                            .to_string();
                        matches.push(display_path);
                    }
                    Err(e) => {
                        debug!(error = %e, "glob entry error");
                    }
                }
            }

            matches.sort();

            if matches.is_empty() {
                return Ok(ToolOutput {
                    content: format!("No files found matching pattern: {pattern_str}"),
                    success: true,
                });
            }

            let mut output = format!("Found {} file(s):\n", matches.len());
            for m in &matches {
                output.push_str(m);
                output.push('\n');
            }

            if truncated {
                output.push_str(&format!(
                    "\n(Results truncated at {MAX_RESULTS}. Narrow your pattern.)"
                ));
            }

            Ok(ToolOutput {
                content: output,
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

    #[tokio::test]
    async fn glob_finds_files() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        std::fs::write(dir.path().join("b.txt"), "").unwrap();
        std::fs::write(dir.path().join("c.rs"), "").unwrap();

        let tool = GlobSearchTool;
        let ctx = make_ctx(dir.path());
        let params = serde_json::json!({"pattern": "*.txt"});
        let result = tool.execute(params, &ctx).await.unwrap();
        assert!(result.success);
        assert!(result.content.contains("a.txt"));
        assert!(result.content.contains("b.txt"));
        assert!(!result.content.contains("c.rs"));
    }

    #[tokio::test]
    async fn glob_recursive() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("src/nested")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "").unwrap();
        std::fs::write(dir.path().join("src/nested/lib.rs"), "").unwrap();
        std::fs::write(dir.path().join("readme.md"), "").unwrap();

        let tool = GlobSearchTool;
        let ctx = make_ctx(dir.path());
        let params = serde_json::json!({"pattern": "**/*.rs"});
        let result = tool.execute(params, &ctx).await.unwrap();
        assert!(result.success);
        assert!(result.content.contains("main.rs"));
        assert!(result.content.contains("lib.rs"));
        assert!(!result.content.contains("readme.md"));
    }

    #[tokio::test]
    async fn glob_no_matches() {
        let dir = TempDir::new().unwrap();
        let tool = GlobSearchTool;
        let ctx = make_ctx(dir.path());
        let params = serde_json::json!({"pattern": "*.nonexistent"});
        let result = tool.execute(params, &ctx).await.unwrap();
        assert!(result.success);
        assert!(result.content.contains("No files found"));
    }

    #[tokio::test]
    async fn glob_with_path_parameter() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/test.txt"), "").unwrap();
        std::fs::write(dir.path().join("top.txt"), "").unwrap();

        let tool = GlobSearchTool;
        let ctx = make_ctx(dir.path());
        let params = serde_json::json!({"pattern": "*.txt", "path": "sub"});
        let result = tool.execute(params, &ctx).await.unwrap();
        assert!(result.success);
        assert!(result.content.contains("test.txt"));
        assert!(!result.content.contains("top.txt"));
    }

    #[tokio::test]
    async fn glob_missing_pattern() {
        let dir = TempDir::new().unwrap();
        let tool = GlobSearchTool;
        let ctx = make_ctx(dir.path());
        let params = serde_json::json!({});
        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
    }
}
