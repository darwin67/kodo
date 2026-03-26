use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use tracing::debug;

/// A snapshot of a file's content at a point in time.
#[derive(Debug, Clone)]
struct Snapshot {
    /// The file content before the edit. `None` means the file did not exist.
    content: Option<String>,
}

/// Manages file checkpoints for undo support.
///
/// Before a file is modified, its current content is saved as a snapshot.
/// The user can then undo the most recent edit to any file, or undo all edits.
///
/// Snapshots are stored in memory (not persisted to disk in this phase).
pub struct CheckpointManager {
    /// Stack of snapshots per file path. Most recent snapshot is last.
    snapshots: HashMap<PathBuf, Vec<Snapshot>>,
    /// Ordered list of checkpointed paths for undo_last.
    order: Vec<PathBuf>,
}

impl CheckpointManager {
    pub fn new() -> Self {
        Self {
            snapshots: HashMap::new(),
            order: Vec::new(),
        }
    }

    /// Normalize a path for consistent HashMap lookups.
    /// Canonicalizes the parent directory (which should exist) and appends the filename.
    /// This handles macOS /tmp -> /private/var/... symlinks.
    fn normalize_path(path: &Path) -> PathBuf {
        if let Some(parent) = path.parent()
            && parent.exists()
            && let Ok(canonical_parent) = parent.canonicalize()
            && let Some(file_name) = path.file_name()
        {
            return canonical_parent.join(file_name);
        }
        path.to_path_buf()
    }

    /// Take a snapshot of the file before modifying it.
    /// Call this before every file_write or file_edit.
    pub async fn snapshot(&mut self, path: &Path) -> Result<()> {
        let key = Self::normalize_path(path);

        let content = if key.exists() {
            Some(tokio::fs::read_to_string(&key).await?)
        } else {
            None
        };

        debug!(path = %key.display(), existed = content.is_some(), "checkpoint saved");

        self.snapshots
            .entry(key.clone())
            .or_default()
            .push(Snapshot { content });
        self.order.push(key);

        Ok(())
    }

    /// Undo the most recent edit to a specific file.
    /// Returns a description of what was restored.
    pub async fn undo_file(&mut self, path: &Path) -> Result<String> {
        let key = Self::normalize_path(path);

        let stack = match self.snapshots.get_mut(&key) {
            Some(s) if !s.is_empty() => s,
            _ => bail!("no checkpoints for {}", key.display()),
        };

        let snapshot = stack.pop().unwrap();

        // Remove this entry from the order list.
        if let Some(pos) = self.order.iter().rposition(|p| p == &key) {
            self.order.remove(pos);
        }

        match snapshot.content {
            Some(content) => {
                tokio::fs::write(&key, &content).await?;
                debug!(path = %key.display(), "restored from checkpoint");
                Ok(format!(
                    "Restored {} ({} bytes)",
                    key.display(),
                    content.len()
                ))
            }
            None => {
                // File didn't exist before — remove it.
                if key.exists() {
                    tokio::fs::remove_file(&key).await?;
                }
                debug!(path = %key.display(), "removed file (didn't exist before)");
                Ok(format!(
                    "Removed {} (file was newly created)",
                    key.display()
                ))
            }
        }
    }

    /// Undo the most recent edit across all files.
    pub async fn undo_last(&mut self) -> Result<String> {
        let path = self
            .order
            .last()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no checkpoints to undo"))?;

        self.undo_file(&path).await
    }

    /// Number of total checkpoints across all files.
    pub fn checkpoint_count(&self) -> usize {
        self.snapshots.values().map(|s| s.len()).sum()
    }

    /// Number of files with at least one checkpoint.
    pub fn files_with_checkpoints(&self) -> usize {
        self.snapshots.values().filter(|s| !s.is_empty()).count()
    }

    /// List all files that have checkpoints, with their checkpoint counts.
    pub fn list_checkpoints(&self) -> Vec<(PathBuf, usize)> {
        self.snapshots
            .iter()
            .filter(|(_, stack)| !stack.is_empty())
            .map(|(path, stack)| (path.clone(), stack.len()))
            .collect()
    }
}

impl Default for CheckpointManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn snapshot_and_undo_existing_file() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "original").unwrap();

        let mut mgr = CheckpointManager::new();

        mgr.snapshot(&file).await.unwrap();
        std::fs::write(&file, "modified").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "modified");

        let msg = mgr.undo_file(&file).await.unwrap();
        assert!(msg.contains("Restored"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "original");
    }

    #[tokio::test]
    async fn snapshot_and_undo_new_file() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("new.txt");

        let mut mgr = CheckpointManager::new();

        mgr.snapshot(&file).await.unwrap();
        std::fs::write(&file, "new content").unwrap();
        assert!(file.exists());

        let msg = mgr.undo_file(&file).await.unwrap();
        assert!(msg.contains("Removed"));
        // The canonical path may differ on macOS, so check via normalize.
        let normalized = CheckpointManager::normalize_path(&file);
        assert!(!normalized.exists());
    }

    #[tokio::test]
    async fn multiple_snapshots_undo_in_order() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "v1").unwrap();

        let mut mgr = CheckpointManager::new();

        mgr.snapshot(&file).await.unwrap();
        std::fs::write(&file, "v2").unwrap();

        mgr.snapshot(&file).await.unwrap();
        std::fs::write(&file, "v3").unwrap();

        mgr.undo_file(&file).await.unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "v2");

        mgr.undo_file(&file).await.unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "v1");
    }

    #[tokio::test]
    async fn undo_with_no_checkpoints_fails() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.txt");
        let mut mgr = CheckpointManager::new();
        let result = mgr.undo_file(&file).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no checkpoints"));
    }

    #[tokio::test]
    async fn undo_last_works() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "original").unwrap();

        let mut mgr = CheckpointManager::new();
        mgr.snapshot(&file).await.unwrap();
        std::fs::write(&file, "modified").unwrap();

        let msg = mgr.undo_last().await.unwrap();
        assert!(msg.contains("Restored"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "original");
    }

    #[tokio::test]
    async fn undo_last_with_no_checkpoints_fails() {
        let mut mgr = CheckpointManager::new();
        let result = mgr.undo_last().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn undo_last_respects_order() {
        let dir = TempDir::new().unwrap();
        let file_a = dir.path().join("a.txt");
        let file_b = dir.path().join("b.txt");
        std::fs::write(&file_a, "a-original").unwrap();
        std::fs::write(&file_b, "b-original").unwrap();

        let mut mgr = CheckpointManager::new();

        mgr.snapshot(&file_a).await.unwrap();
        std::fs::write(&file_a, "a-modified").unwrap();

        mgr.snapshot(&file_b).await.unwrap();
        std::fs::write(&file_b, "b-modified").unwrap();

        // undo_last should undo file_b first (most recent).
        mgr.undo_last().await.unwrap();
        assert_eq!(std::fs::read_to_string(&file_b).unwrap(), "b-original");
        assert_eq!(std::fs::read_to_string(&file_a).unwrap(), "a-modified");

        // undo_last should undo file_a next.
        mgr.undo_last().await.unwrap();
        assert_eq!(std::fs::read_to_string(&file_a).unwrap(), "a-original");
    }

    #[tokio::test]
    async fn checkpoint_count() {
        let dir = TempDir::new().unwrap();
        let f1 = dir.path().join("a.txt");
        let f2 = dir.path().join("b.txt");
        std::fs::write(&f1, "a").unwrap();
        std::fs::write(&f2, "b").unwrap();

        let mut mgr = CheckpointManager::new();
        assert_eq!(mgr.checkpoint_count(), 0);
        assert_eq!(mgr.files_with_checkpoints(), 0);

        mgr.snapshot(&f1).await.unwrap();
        mgr.snapshot(&f2).await.unwrap();
        mgr.snapshot(&f1).await.unwrap();

        assert_eq!(mgr.checkpoint_count(), 3);
        assert_eq!(mgr.files_with_checkpoints(), 2);
    }

    #[tokio::test]
    async fn list_checkpoints() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "content").unwrap();

        let mut mgr = CheckpointManager::new();
        mgr.snapshot(&file).await.unwrap();
        mgr.snapshot(&file).await.unwrap();

        let list = mgr.list_checkpoints();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].1, 2);
    }
}
