use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use tracing::{debug, warn};

use crate::config::{LspServerConfig, builtin_configs, is_command_available};
use crate::diagnostics::{self, Diagnostic};
use crate::transport::Transport;

/// Manages LSP server connections, routing by file extension.
pub struct LspManager {
    /// Map from server name to active transport.
    servers: HashMap<String, Transport>,
    /// Map from file extension to server config.
    extension_map: HashMap<String, LspServerConfig>,
    /// Project root directory.
    root_dir: PathBuf,
    /// File version counter for textDocument/didOpen and didChange.
    versions: HashMap<String, i32>,
}

impl LspManager {
    /// Create a new manager for the given project root.
    /// Registers built-in server configs that are available on PATH.
    pub fn new(root_dir: PathBuf) -> Self {
        let mut extension_map = HashMap::new();

        for config in builtin_configs() {
            if is_command_available(config.command) {
                debug!(server = config.name, "LSP server available");
                for ext in config.extensions {
                    extension_map.insert(ext.to_string(), config.clone());
                }
            } else {
                debug!(server = config.name, "LSP server not found on PATH");
            }
        }

        Self {
            servers: HashMap::new(),
            extension_map,
            root_dir,
            versions: HashMap::new(),
        }
    }

    /// Check if there is a configured LSP server for the given file.
    pub fn has_server_for(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| self.extension_map.contains_key(ext))
    }

    /// Ensure the LSP server for a file extension is running.
    /// Spawns and initializes the server if not already active.
    pub async fn ensure_server(&mut self, path: &Path) -> Result<&str> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .ok_or_else(|| anyhow::anyhow!("file has no extension"))?;

        let config = self
            .extension_map
            .get(ext)
            .ok_or_else(|| anyhow::anyhow!("no LSP server configured for .{ext}"))?
            .clone();

        if self.servers.contains_key(config.name) {
            return Ok(config.name);
        }

        debug!(server = config.name, "starting LSP server");

        let args: Vec<&str> = config.args.to_vec();
        let root_str = self.root_dir.to_string_lossy().to_string();

        let mut transport = Transport::spawn(config.command, &args, &root_str).await?;

        // Send initialize request.
        let root_uri = format!("file://{}", self.root_dir.display());
        let init_params = serde_json::json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "capabilities": {
                "textDocument": {
                    "publishDiagnostics": {
                        "relatedInformation": true
                    },
                    "synchronization": {
                        "didOpen": true,
                        "didChange": true
                    }
                }
            }
        });

        let _init_result = transport.request("initialize", init_params).await?;

        // Send initialized notification.
        transport
            .notify("initialized", serde_json::json!({}))
            .await?;

        debug!(server = config.name, "LSP server initialized");

        let name = config.name;
        self.servers.insert(name.to_string(), transport);

        Ok(name)
    }

    /// Notify the LSP that a file was opened.
    pub async fn did_open(&mut self, path: &Path, content: &str) -> Result<()> {
        let server_name = self.ensure_server(path).await?.to_string();
        let uri = path_to_uri(path);
        let language_id = detect_language(path);

        let version = self.next_version(&uri);

        if let Some(transport) = self.servers.get_mut(&server_name) {
            transport
                .notify(
                    "textDocument/didOpen",
                    serde_json::json!({
                        "textDocument": {
                            "uri": uri,
                            "languageId": language_id,
                            "version": version,
                            "text": content
                        }
                    }),
                )
                .await?;
        }

        Ok(())
    }

    /// Notify the LSP that a file changed (full content sync).
    pub async fn did_change(&mut self, path: &Path, content: &str) -> Result<()> {
        let server_name = match self.server_name_for(path) {
            Some(name) => name.to_string(),
            None => self.ensure_server(path).await?.to_string(),
        };

        let uri = path_to_uri(path);
        let version = self.next_version(&uri);

        if let Some(transport) = self.servers.get_mut(&server_name) {
            transport
                .notify(
                    "textDocument/didChange",
                    serde_json::json!({
                        "textDocument": {
                            "uri": uri,
                            "version": version
                        },
                        "contentChanges": [{
                            "text": content
                        }]
                    }),
                )
                .await?;
        }

        Ok(())
    }

    /// Collect any pending diagnostics from all active servers.
    pub async fn collect_diagnostics(&mut self) -> Vec<Diagnostic> {
        let mut all_diagnostics = Vec::new();

        for (name, transport) in &mut self.servers {
            let notifications = transport.read_notifications().await;
            for params in notifications {
                let diags = diagnostics::parse_diagnostics(&params);
                if !diags.is_empty() {
                    debug!(server = %name, count = diags.len(), "collected diagnostics");
                    all_diagnostics.extend(diags);
                }
            }
        }

        all_diagnostics
    }

    /// Collect diagnostics for a specific file after notifying of a change.
    /// Waits briefly for the server to process the change.
    pub async fn diagnostics_after_change(
        &mut self,
        path: &Path,
        content: &str,
    ) -> Result<Vec<Diagnostic>> {
        self.did_change(path, content).await?;

        // Give the server a moment to produce diagnostics.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        Ok(self.collect_diagnostics().await)
    }

    /// Shut down all active LSP servers.
    pub async fn shutdown_all(&mut self) {
        for (name, transport) in &mut self.servers {
            debug!(server = %name, "shutting down LSP server");
            if let Err(e) = transport.shutdown().await {
                warn!(server = %name, error = %e, "error shutting down LSP server");
            }
        }
        self.servers.clear();
    }

    /// Number of active LSP servers.
    pub fn active_server_count(&self) -> usize {
        self.servers.len()
    }

    /// List names of configured servers (available on PATH).
    pub fn available_servers(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .extension_map
            .values()
            .map(|c| c.name)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        names.sort();
        names
    }

    fn server_name_for(&self, path: &Path) -> Option<&str> {
        let ext = path.extension()?.to_str()?;
        let config = self.extension_map.get(ext)?;
        if self.servers.contains_key(config.name) {
            Some(config.name)
        } else {
            None
        }
    }

    fn next_version(&mut self, uri: &str) -> i32 {
        let version = self.versions.entry(uri.to_string()).or_insert(0);
        *version += 1;
        *version
    }
}

fn path_to_uri(path: &Path) -> String {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    // URIs always use forward slashes, even on Windows
    let uri_path = abs.to_string_lossy().replace('\\', "/");
    format!("file://{}", uri_path)
}

fn detect_language(path: &Path) -> &str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("rs") => "rust",
        Some("go") => "go",
        Some("ts") => "typescript",
        Some("tsx") => "typescriptreact",
        Some("js") => "javascript",
        Some("jsx") => "javascriptreact",
        Some("py" | "pyi") => "python",
        _ => "plaintext",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_to_uri_absolute() {
        let path = if cfg!(windows) {
            Path::new("C:\\tmp\\test.rs")
        } else {
            Path::new("/tmp/test.rs")
        };
        let uri = path_to_uri(path);

        let expected = if cfg!(windows) {
            "file://C:/tmp/test.rs"
        } else {
            "file:///tmp/test.rs"
        };

        assert_eq!(uri, expected);
    }

    #[test]
    fn detect_language_rust() {
        assert_eq!(detect_language(Path::new("main.rs")), "rust");
    }

    #[test]
    fn detect_language_go() {
        assert_eq!(detect_language(Path::new("main.go")), "go");
    }

    #[test]
    fn detect_language_typescript() {
        assert_eq!(detect_language(Path::new("app.ts")), "typescript");
        assert_eq!(detect_language(Path::new("app.tsx")), "typescriptreact");
    }

    #[test]
    fn detect_language_python() {
        assert_eq!(detect_language(Path::new("script.py")), "python");
    }

    #[test]
    fn detect_language_unknown() {
        assert_eq!(detect_language(Path::new("Makefile")), "plaintext");
    }

    #[test]
    fn manager_available_servers() {
        let mgr = LspManager::new(PathBuf::from("/tmp"));
        // Just verify it doesn't crash; available servers depend on environment.
        let _servers = mgr.available_servers();
    }

    #[test]
    fn manager_has_server_for() {
        let mgr = LspManager::new(PathBuf::from("/tmp"));
        // Only returns true if the server binary is on PATH.
        // We can't guarantee any specific server is installed.
        let _ = mgr.has_server_for(Path::new("test.rs"));
        let _ = mgr.has_server_for(Path::new("test.xyz"));
    }
}
