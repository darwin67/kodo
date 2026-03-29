use std::path::Path;

use anyhow::Result;
use tracing::debug;

/// Load the contents of a KODO.md file from the given directory.
///
/// Searches for KODO.md (case-insensitive) in the project root.
/// Returns `None` if no file is found.
pub async fn load_project_memory(directory: &Path) -> Result<Option<String>> {
    // Try common names.
    for name in &["KODO.md", "kodo.md", ".kodo.md"] {
        let path = directory.join(name);
        if path.exists() {
            debug!(path = %path.display(), "loading project memory");
            let content = tokio::fs::read_to_string(&path).await?;
            return Ok(Some(content));
        }
    }
    Ok(None)
}

/// Build a system prompt supplement from KODO.md content.
///
/// This gets appended to the system prompt at session start.
pub fn format_project_memory(content: &str) -> String {
    format!(
        "\n\n--- Project Instructions (from KODO.md) ---\n\n{content}\n\n--- End Project Instructions ---"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn load_kodo_md() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("KODO.md"), "Always use tabs.\n").unwrap();

        let content = load_project_memory(dir.path()).await.unwrap();
        assert!(content.is_some());
        assert!(content.unwrap().contains("Always use tabs"));
    }

    #[tokio::test]
    async fn load_lowercase_kodo_md() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("kodo.md"), "Use Rust.\n").unwrap();

        let content = load_project_memory(dir.path()).await.unwrap();
        assert!(content.is_some());
    }

    #[tokio::test]
    async fn load_no_kodo_md() {
        let dir = TempDir::new().unwrap();
        let content = load_project_memory(dir.path()).await.unwrap();
        assert!(content.is_none());
    }

    #[test]
    fn format_memory_wraps_content() {
        let formatted = format_project_memory("Use tabs.");
        assert!(formatted.contains("Use tabs."));
        assert!(formatted.contains("Project Instructions"));
    }
}
