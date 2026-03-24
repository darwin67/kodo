use std::path::PathBuf;

use anyhow::{Result, bail};
use tracing::debug;

use crate::tool::{PermissionLevel, Tool, ToolContext, ToolOutput};

/// Tool that reads a file from disk with optional line range.
pub struct FileReadTool;

impl Tool for FileReadTool {
    fn name(&self) -> &str {
        "file_read"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. Returns the file content with line numbers. \
         Optionally specify offset (0-based line number to start from) and limit \
         (number of lines to read)."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the file to read (absolute or relative to working directory)"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (0-based). Defaults to 0."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to read. Defaults to 2000."
                }
            },
            "required": ["path"]
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
            let path_str = params
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("missing required parameter: path"))?;

            let offset = params.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

            let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(2000) as usize;

            let path = resolve_path(&working_dir, path_str);
            debug!(path = %path.display(), offset, limit, "reading file");

            if !path.exists() {
                bail!("file not found: {}", path.display());
            }

            if !path.is_file() {
                bail!("not a file: {}", path.display());
            }

            let content = tokio::fs::read_to_string(&path).await?;
            let lines: Vec<&str> = content.lines().collect();
            let total_lines = lines.len();

            if offset >= total_lines && total_lines > 0 {
                bail!(
                    "offset {} is beyond end of file ({} lines)",
                    offset,
                    total_lines
                );
            }

            let end = (offset + limit).min(total_lines);
            let selected: Vec<String> = lines[offset..end]
                .iter()
                .enumerate()
                .map(|(i, line)| format!("{:>6}\t{}", offset + i + 1, line))
                .collect();

            let mut output = selected.join("\n");

            if end < total_lines {
                output.push_str(&format!(
                    "\n\n(File has more lines. Use offset={} to read beyond line {}. Total: {} lines)",
                    end, end, total_lines
                ));
            }

            Ok(ToolOutput {
                content: output,
                success: true,
            })
        })
    }
}

/// Resolve a path relative to the working directory.
/// If the path is absolute, return it as-is.
pub(crate) fn resolve_path(working_dir: &PathBuf, path_str: &str) -> PathBuf {
    let path = PathBuf::from(path_str);
    if path.is_absolute() {
        path
    } else {
        working_dir.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn make_ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext {
            working_dir: dir.to_path_buf(),
        }
    }

    fn make_temp_file(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "{}", content).unwrap();
        f
    }

    #[tokio::test]
    async fn read_simple_file() {
        let f = make_temp_file("line 1\nline 2\nline 3\n");
        let tool = FileReadTool;
        let ctx = make_ctx(std::path::Path::new("/tmp"));
        let params = serde_json::json!({"path": f.path().to_str().unwrap()});
        let result = tool.execute(params, &ctx).await.unwrap();
        assert!(result.success);
        assert!(result.content.contains("line 1"));
        assert!(result.content.contains("line 2"));
        assert!(result.content.contains("line 3"));
    }

    #[tokio::test]
    async fn read_with_offset() {
        let f = make_temp_file("a\nb\nc\nd\ne\n");
        let tool = FileReadTool;
        let ctx = make_ctx(std::path::Path::new("/tmp"));
        let params = serde_json::json!({"path": f.path().to_str().unwrap(), "offset": 2});
        let result = tool.execute(params, &ctx).await.unwrap();
        assert!(result.success);
        // Should start from line 3 (0-indexed offset 2)
        assert!(result.content.contains("\tc"));
        assert!(result.content.contains("\td"));
        assert!(!result.content.contains("\ta"));
        assert!(!result.content.contains("\tb"));
    }

    #[tokio::test]
    async fn read_with_limit() {
        let f = make_temp_file("a\nb\nc\nd\ne\n");
        let tool = FileReadTool;
        let ctx = make_ctx(std::path::Path::new("/tmp"));
        let params = serde_json::json!({"path": f.path().to_str().unwrap(), "limit": 2});
        let result = tool.execute(params, &ctx).await.unwrap();
        assert!(result.success);
        assert!(result.content.contains("\ta"));
        assert!(result.content.contains("\tb"));
        assert!(result.content.contains("File has more lines"));
    }

    #[tokio::test]
    async fn read_nonexistent_file() {
        let tool = FileReadTool;
        let ctx = make_ctx(std::path::Path::new("/tmp"));
        let params = serde_json::json!({"path": "/tmp/kodo_nonexistent_12345.txt"});
        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("file not found"));
    }

    #[tokio::test]
    async fn read_missing_path_param() {
        let tool = FileReadTool;
        let ctx = make_ctx(std::path::Path::new("/tmp"));
        let params = serde_json::json!({});
        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
    }

    #[test]
    fn resolve_absolute_path() {
        let wd = PathBuf::from("/home/user/project");
        let resolved = resolve_path(&wd, "/etc/hosts");
        assert_eq!(resolved, PathBuf::from("/etc/hosts"));
    }

    #[test]
    fn resolve_relative_path() {
        let wd = PathBuf::from("/home/user/project");
        let resolved = resolve_path(&wd, "src/main.rs");
        assert_eq!(resolved, PathBuf::from("/home/user/project/src/main.rs"));
    }

    #[tokio::test]
    async fn read_has_line_numbers() {
        let f = make_temp_file("hello\nworld\n");
        let tool = FileReadTool;
        let ctx = make_ctx(std::path::Path::new("/tmp"));
        let params = serde_json::json!({"path": f.path().to_str().unwrap()});
        let result = tool.execute(params, &ctx).await.unwrap();
        // Line numbers should be 1-based
        assert!(result.content.contains("     1\t"));
        assert!(result.content.contains("     2\t"));
    }
}
