use std::io::BufRead;

use anyhow::Result;
use tracing::debug;

use crate::file_read::resolve_path;
use crate::tool::{PermissionLevel, Tool, ToolContext, ToolOutput};

const MAX_MATCHES: usize = 200;

/// Tool that searches file contents using regex.
pub struct GrepSearchTool;

impl Tool for GrepSearchTool {
    fn name(&self) -> &str {
        "grep_search"
    }

    fn description(&self) -> &str {
        "Search file contents using a regular expression. Searches all files in the \
         working directory (or a specified path) recursively. Returns matching lines \
         with file paths and line numbers."
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

            let search_path = params
                .get("path")
                .and_then(|v| v.as_str())
                .map(|p| resolve_path(&working_dir, p))
                .unwrap_or_else(|| working_dir.clone());

            let include_pattern = params.get("include").and_then(|v| v.as_str());

            debug!(pattern = %pattern_str, path = %search_path.display(), "grep search");

            let re = regex::Regex::new(pattern_str)
                .map_err(|e| anyhow::anyhow!("invalid regex: {e}"))?;

            // Collect file list with glob include filter.
            let file_glob = if let Some(include) = include_pattern {
                let glob_pattern = search_path.join("**").join(include);
                glob_pattern.to_string_lossy().to_string()
            } else {
                search_path.join("**/*").to_string_lossy().to_string()
            };

            let mut matches: Vec<String> = Vec::new();
            let mut files_with_matches = 0usize;
            let mut truncated = false;

            for entry in glob::glob(&file_glob).unwrap_or_else(|_| glob::glob("").unwrap()) {
                let path = match entry {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                if !path.is_file() {
                    continue;
                }

                // Skip binary files by checking if the file opens as valid text.
                let file = match std::fs::File::open(&path) {
                    Ok(f) => f,
                    Err(_) => continue,
                };

                let reader = std::io::BufReader::new(file);
                let mut file_had_match = false;

                for (line_num, line) in reader.lines().enumerate() {
                    let line = match line {
                        Ok(l) => l,
                        Err(_) => break, // Likely binary file
                    };

                    if re.is_match(&line) {
                        if matches.len() >= MAX_MATCHES {
                            truncated = true;
                            break;
                        }

                        let display_path = path
                            .strip_prefix(&working_dir)
                            .unwrap_or(&path)
                            .to_string_lossy();

                        matches.push(format!("{}:{}: {}", display_path, line_num + 1, line));
                        file_had_match = true;
                    }
                }

                if file_had_match {
                    files_with_matches += 1;
                }

                if truncated {
                    break;
                }
            }

            if matches.is_empty() {
                return Ok(ToolOutput {
                    content: format!("No matches found for pattern: {pattern_str}"),
                    success: true,
                });
            }

            let mut output = format!(
                "Found {} match(es) in {} file(s):\n\n",
                matches.len(),
                files_with_matches
            );
            output.push_str(&matches.join("\n"));

            if truncated {
                output.push_str(&format!(
                    "\n\n(Results truncated at {MAX_MATCHES} matches. Narrow your search.)"
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
    async fn grep_finds_matches() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("test.txt"),
            "hello world\nfoo bar\nhello again\n",
        )
        .unwrap();

        let tool = GrepSearchTool;
        let ctx = make_ctx(dir.path());
        let params = serde_json::json!({"pattern": "hello"});
        let result = tool.execute(params, &ctx).await.unwrap();
        assert!(result.success);
        assert!(result.content.contains("2 match(es)"));
        assert!(result.content.contains("hello world"));
        assert!(result.content.contains("hello again"));
    }

    #[tokio::test]
    async fn grep_with_regex() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("test.rs"),
            "fn main() {\nfn helper() {\nlet x = 1;\n",
        )
        .unwrap();

        let tool = GrepSearchTool;
        let ctx = make_ctx(dir.path());
        let params = serde_json::json!({"pattern": r"fn\s+\w+"});
        let result = tool.execute(params, &ctx).await.unwrap();
        assert!(result.success);
        assert!(result.content.contains("fn main"));
        assert!(result.content.contains("fn helper"));
    }

    #[tokio::test]
    async fn grep_no_matches() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello world\n").unwrap();

        let tool = GrepSearchTool;
        let ctx = make_ctx(dir.path());
        let params = serde_json::json!({"pattern": "nonexistent"});
        let result = tool.execute(params, &ctx).await.unwrap();
        assert!(result.success);
        assert!(result.content.contains("No matches found"));
    }

    #[tokio::test]
    async fn grep_with_include_filter() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("code.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.path().join("readme.md"), "fn not_code\n").unwrap();

        let tool = GrepSearchTool;
        let ctx = make_ctx(dir.path());
        let params = serde_json::json!({"pattern": "fn", "include": "*.rs"});
        let result = tool.execute(params, &ctx).await.unwrap();
        assert!(result.success);
        assert!(result.content.contains("code.rs"));
        assert!(!result.content.contains("readme.md"));
    }

    #[tokio::test]
    async fn grep_invalid_regex() {
        let dir = TempDir::new().unwrap();
        let tool = GrepSearchTool;
        let ctx = make_ctx(dir.path());
        let params = serde_json::json!({"pattern": "[invalid"});
        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid regex"));
    }

    #[tokio::test]
    async fn grep_shows_line_numbers() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("test.txt"), "aaa\nbbb\nccc\nbbb\n").unwrap();

        let tool = GrepSearchTool;
        let ctx = make_ctx(dir.path());
        let params = serde_json::json!({"pattern": "bbb"});
        let result = tool.execute(params, &ctx).await.unwrap();
        assert!(result.success);
        assert!(result.content.contains(":2:"));
        assert!(result.content.contains(":4:"));
    }
}
