use anyhow::{Result, bail};
use tracing::debug;

use crate::file_read::resolve_path;
use crate::tool::{PermissionLevel, Tool, ToolContext, ToolOutput};

/// Tool that edits a file by replacing an exact string match.
pub struct FileEditTool;

impl Tool for FileEditTool {
    fn name(&self) -> &str {
        "file_edit"
    }

    fn description(&self) -> &str {
        "Edit a file by replacing an exact string match. The oldString must appear exactly \
         once in the file (unless replaceAll is true). Use this for precise, surgical edits."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the file to edit"
                },
                "old_string": {
                    "type": "string",
                    "description": "The exact string to find and replace"
                },
                "new_string": {
                    "type": "string",
                    "description": "The replacement string"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "If true, replace all occurrences. Default false."
                }
            },
            "required": ["path", "old_string", "new_string"]
        })
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Write
    }

    fn execute(
        &self,
        params: serde_json::Value,
        ctx: &ToolContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ToolOutput>> + Send + '_>> {
        let working_dir = ctx.working_dir.clone();
        Box::pin(async move {
            let path_str = params
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("missing required parameter: path"))?;

            let old_string = params
                .get("old_string")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("missing required parameter: old_string"))?;

            let new_string = params
                .get("new_string")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("missing required parameter: new_string"))?;

            let replace_all = params
                .get("replace_all")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if old_string == new_string {
                bail!("old_string and new_string are identical; no edit needed");
            }

            let path = resolve_path(&working_dir, path_str);
            debug!(path = %path.display(), replace_all, "editing file");

            if !path.exists() {
                bail!("file not found: {}", path.display());
            }

            let content = tokio::fs::read_to_string(&path).await?;

            let match_count = content.matches(old_string).count();
            if match_count == 0 {
                bail!("old_string not found in {}", path.display());
            }

            if !replace_all && match_count > 1 {
                bail!(
                    "old_string found {} times in {}. Provide more context to make \
                     the match unique, or set replace_all to true.",
                    match_count,
                    path.display()
                );
            }

            let new_content = if replace_all {
                content.replace(old_string, new_string)
            } else {
                content.replacen(old_string, new_string, 1)
            };

            tokio::fs::write(&path, &new_content).await?;

            Ok(ToolOutput {
                content: format!(
                    "Replaced {} occurrence(s) in {}",
                    if replace_all { match_count } else { 1 },
                    path.display()
                ),
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
    async fn edit_single_replacement() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "hello world").unwrap();

        let tool = FileEditTool;
        let ctx = make_ctx(dir.path());
        let params = serde_json::json!({
            "path": file_path.to_str().unwrap(),
            "old_string": "world",
            "new_string": "rust"
        });

        let result = tool.execute(params, &ctx).await.unwrap();
        assert!(result.success);
        assert!(result.content.contains("1 occurrence"));

        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "hello rust");
    }

    #[tokio::test]
    async fn edit_replace_all() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "aaa bbb aaa ccc aaa").unwrap();

        let tool = FileEditTool;
        let ctx = make_ctx(dir.path());
        let params = serde_json::json!({
            "path": file_path.to_str().unwrap(),
            "old_string": "aaa",
            "new_string": "xxx",
            "replace_all": true
        });

        let result = tool.execute(params, &ctx).await.unwrap();
        assert!(result.success);
        assert!(result.content.contains("3 occurrence"));

        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "xxx bbb xxx ccc xxx");
    }

    #[tokio::test]
    async fn edit_fails_on_ambiguous_match() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "foo bar foo").unwrap();

        let tool = FileEditTool;
        let ctx = make_ctx(dir.path());
        let params = serde_json::json!({
            "path": file_path.to_str().unwrap(),
            "old_string": "foo",
            "new_string": "baz"
        });

        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("found 2 times"));
    }

    #[tokio::test]
    async fn edit_fails_on_not_found() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "hello world").unwrap();

        let tool = FileEditTool;
        let ctx = make_ctx(dir.path());
        let params = serde_json::json!({
            "path": file_path.to_str().unwrap(),
            "old_string": "nonexistent",
            "new_string": "replacement"
        });

        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn edit_fails_on_identical_strings() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "hello world").unwrap();

        let tool = FileEditTool;
        let ctx = make_ctx(dir.path());
        let params = serde_json::json!({
            "path": file_path.to_str().unwrap(),
            "old_string": "hello",
            "new_string": "hello"
        });

        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("identical"));
    }

    #[tokio::test]
    async fn edit_nonexistent_file() {
        let dir = TempDir::new().unwrap();
        let tool = FileEditTool;
        let ctx = make_ctx(dir.path());
        let params = serde_json::json!({
            "path": "/tmp/kodo_nonexistent_edit_test.txt",
            "old_string": "a",
            "new_string": "b"
        });

        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn edit_multiline_replacement() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.rs");
        std::fs::write(&file_path, "fn main() {\n    println!(\"hello\");\n}\n").unwrap();

        let tool = FileEditTool;
        let ctx = make_ctx(dir.path());
        let params = serde_json::json!({
            "path": file_path.to_str().unwrap(),
            "old_string": "    println!(\"hello\");",
            "new_string": "    println!(\"goodbye\");\n    println!(\"world\");"
        });

        let result = tool.execute(params, &ctx).await.unwrap();
        assert!(result.success);

        let content = std::fs::read_to_string(&file_path).unwrap();
        assert!(content.contains("goodbye"));
        assert!(content.contains("world"));
    }
}
