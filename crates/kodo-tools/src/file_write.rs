use anyhow::Result;
use tracing::debug;

use crate::file_read::resolve_path;
use crate::tool::{PermissionLevel, Tool, ToolContext, ToolOutput};

/// Tool that writes content to a file, creating parent directories as needed.
pub struct FileWriteTool;

impl Tool for FileWriteTool {
    fn name(&self) -> &str {
        "file_write"
    }

    fn description(&self) -> &str {
        "Write content to a file. Creates the file if it doesn't exist, or overwrites it \
         if it does. Parent directories are created automatically."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the file to write (absolute or relative to working directory)"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                }
            },
            "required": ["path", "content"]
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

            let content = params
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("missing required parameter: content"))?;

            let path = resolve_path(&working_dir, path_str);
            debug!(path = %path.display(), bytes = content.len(), "writing file");

            // Create parent directories if they don't exist.
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }

            tokio::fs::write(&path, content).await?;

            let line_count = content.lines().count();
            Ok(ToolOutput {
                content: format!(
                    "Wrote {} bytes ({} lines) to {}",
                    content.len(),
                    line_count,
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
    async fn write_new_file() {
        let dir = TempDir::new().unwrap();
        let tool = FileWriteTool;
        let ctx = make_ctx(dir.path());
        let file_path = dir.path().join("test.txt");

        let params = serde_json::json!({
            "path": file_path.to_str().unwrap(),
            "content": "hello world\n"
        });

        let result = tool.execute(params, &ctx).await.unwrap();
        assert!(result.success);
        assert!(result.content.contains("12 bytes"));

        let written = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(written, "hello world\n");
    }

    #[tokio::test]
    async fn write_creates_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let tool = FileWriteTool;
        let ctx = make_ctx(dir.path());
        let file_path = dir.path().join("a/b/c/deep.txt");

        let params = serde_json::json!({
            "path": file_path.to_str().unwrap(),
            "content": "deep content"
        });

        let result = tool.execute(params, &ctx).await.unwrap();
        assert!(result.success);

        let written = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(written, "deep content");
    }

    #[tokio::test]
    async fn write_overwrites_existing() {
        let dir = TempDir::new().unwrap();
        let tool = FileWriteTool;
        let ctx = make_ctx(dir.path());
        let file_path = dir.path().join("existing.txt");

        std::fs::write(&file_path, "old content").unwrap();

        let params = serde_json::json!({
            "path": file_path.to_str().unwrap(),
            "content": "new content"
        });

        let result = tool.execute(params, &ctx).await.unwrap();
        assert!(result.success);

        let written = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(written, "new content");
    }

    #[tokio::test]
    async fn write_relative_path() {
        let dir = TempDir::new().unwrap();
        let tool = FileWriteTool;
        let ctx = make_ctx(dir.path());

        let params = serde_json::json!({
            "path": "relative.txt",
            "content": "relative content"
        });

        let result = tool.execute(params, &ctx).await.unwrap();
        assert!(result.success);

        let written = std::fs::read_to_string(dir.path().join("relative.txt")).unwrap();
        assert_eq!(written, "relative content");
    }

    #[tokio::test]
    async fn write_missing_params() {
        let dir = TempDir::new().unwrap();
        let tool = FileWriteTool;
        let ctx = make_ctx(dir.path());

        let result = tool.execute(serde_json::json!({}), &ctx).await;
        assert!(result.is_err());

        let result = tool
            .execute(serde_json::json!({"path": "test.txt"}), &ctx)
            .await;
        assert!(result.is_err());
    }
}
