use std::process::Stdio;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tracing::debug;

/// A JSON-RPC transport over stdio to an LSP server process.
pub struct Transport {
    child: Child,
    stdin: tokio::process::ChildStdin,
    reader: BufReader<tokio::process::ChildStdout>,
    next_id: i64,
}

impl Transport {
    /// Spawn an LSP server process and connect via stdio.
    pub async fn spawn(command: &str, args: &[&str], root_dir: &str) -> Result<Self> {
        debug!(command, ?args, root_dir, "spawning LSP server");

        let mut child = Command::new(command)
            .args(args)
            .current_dir(root_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to spawn LSP server: {command}"))?;

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let reader = BufReader::new(stdout);

        Ok(Self {
            child,
            stdin,
            reader,
            next_id: 1,
        })
    }

    /// Send a JSON-RPC request and return the response.
    pub async fn request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let id = self.next_id;
        self.next_id += 1;

        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        self.send_message(&msg).await?;

        // Read responses until we get one matching our request ID.
        loop {
            let response = self.read_message().await?;
            if response.get("id").and_then(|v| v.as_i64()) == Some(id) {
                if let Some(error) = response.get("error") {
                    bail!("LSP error: {}", error);
                }
                return Ok(response
                    .get("result")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null));
            }
            // Not our response — could be a notification or another response.
            // Discard and keep reading.
        }
    }

    /// Send a JSON-RPC notification (no response expected).
    pub async fn notify(&mut self, method: &str, params: serde_json::Value) -> Result<()> {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });

        self.send_message(&msg).await
    }

    /// Read all pending notifications (non-blocking drain).
    /// Returns any `textDocument/publishDiagnostics` params found.
    pub async fn read_notifications(&mut self) -> Vec<serde_json::Value> {
        let mut notifications = Vec::new();

        // Try reading with a short timeout — don't block if nothing is pending.
        while let Ok(Ok(msg)) =
            tokio::time::timeout(std::time::Duration::from_millis(100), self.read_message()).await
        {
            if msg.get("method").and_then(|v| v.as_str()) == Some("textDocument/publishDiagnostics")
                && let Some(params) = msg.get("params")
            {
                notifications.push(params.clone());
            }
            // Continue draining.
        }

        notifications
    }

    /// Send a JSON-RPC message with Content-Length header.
    async fn send_message(&mut self, msg: &serde_json::Value) -> Result<()> {
        let body = serde_json::to_string(msg)?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());

        self.stdin.write_all(header.as_bytes()).await?;
        self.stdin.write_all(body.as_bytes()).await?;
        self.stdin.flush().await?;

        Ok(())
    }

    /// Read a JSON-RPC message (parse Content-Length header, then body).
    async fn read_message(&mut self) -> Result<serde_json::Value> {
        // Read headers until blank line.
        let mut content_length: Option<usize> = None;
        let mut line = String::new();

        loop {
            line.clear();
            self.reader.read_line(&mut line).await?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                break;
            }
            if let Some(len_str) = trimmed.strip_prefix("Content-Length: ") {
                content_length = Some(len_str.parse()?);
            }
        }

        let length =
            content_length.ok_or_else(|| anyhow::anyhow!("missing Content-Length header"))?;

        // Read the body.
        let mut body = vec![0u8; length];
        self.reader.read_exact(&mut body).await?;

        let msg: serde_json::Value = serde_json::from_slice(&body)?;
        Ok(msg)
    }

    /// Check if the server process is still running.
    pub fn is_alive(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_none()
    }

    /// Shut down the server gracefully.
    pub async fn shutdown(&mut self) -> Result<()> {
        // Send shutdown request.
        let _ = self.request("shutdown", serde_json::Value::Null).await;
        // Send exit notification.
        let _ = self.notify("exit", serde_json::Value::Null).await;
        // Wait briefly, then kill if still alive.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if self.is_alive() {
            let _ = self.child.kill().await;
        }
        Ok(())
    }
}
