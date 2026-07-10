// ── Pandoc Server Manager ───────────────────────────────────────────────────
// A process manager for pandoc-server — a background HTTP server for
// converting document formats to Pandoc JSON AST.

use reqwest::Client;
use std::time::Duration;
use tokio::process::Command;

use crate::cfg::RuntimeConfig;
use crate::error::TextRuntimeError;

/// Manages a pandoc-server child process for AST parsing and format conversion.
///
/// The pandoc-server is spawned as a long-lived child process. All conversions
/// go through HTTP to `http://127.0.0.1:{port}/` for single documents and
/// `http://127.0.0.1:{port}/batch` for batch operations.
pub struct PandocManager {
    config: RuntimeConfig,
    client: Client,
    process: Option<tokio::process::Child>,
}

impl PandocManager {
    /// Create a new PandocManager. Does NOT start pandoc-server yet.
    pub fn new(config: RuntimeConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest Client::builder should not fail with default settings");

        Self {
            config,
            client,
            process: None,
        }
    }

    /// Start pandoc-server as a child process.
    ///
    /// Spawns `pandoc-server --port {config.pandoc_port}`, then waits for a
    /// health check via GET http://127.0.0.1:{port}/version. Retries up to
    /// 10 times with exponential backoff (100ms, 200ms, 400ms, ...).
    /// Returns error if server doesn't respond within 10s.
    pub async fn start(&mut self) -> Result<(), TextRuntimeError> {
        // Kill any existing child process before spawning a new one.
        if let Some(mut existing) = self.process.take() {
            let _ = existing.start_kill();
            let _ = existing.wait().await;
        }

        let port = self.config.pandoc_port.to_string();
        let exec = &self.config.pandoc_executable;

        let mut cmd = if exec == "pandoc-server" {
            // Check if pandoc-server exists, otherwise fallback to `pandoc server`
            match which::which("pandoc-server") {
                Ok(_) => Command::new("pandoc-server"),
                Err(_) => {
                    let mut c = Command::new("pandoc");
                    c.arg("server");
                    c
                }
            }
        } else if exec == "pandoc" {
            let mut c = Command::new("pandoc");
            c.arg("server");
            c
        } else {
            Command::new(exec)
        };

        let child = cmd
            .args(["--port", &port])
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                TextRuntimeError::InternalError(format!("failed to spawn pandoc server: {e}"))
            })?;

        self.process = Some(child);

        // Health check with exponential backoff: 100ms * 2^attempt, max 10 attempts.
        //
        // Note: the health check is the contract, NOT whether our own child is
        // still alive. The MCP server intentionally runs ONE shared pandoc
        // server (default port 8499) and the engine is expected to reuse an
        // external server already bound to the configured port — in that case
        // our child exits after a failed bind and the health check correctly
        // answers against the shared server. Tests avoid ambiguity by using a
        // unique ephemeral port per test (see crate::test_util::free_port).
        for attempt in 0..10 {
            let delay_ms = 100u64 * 2u64.pow(attempt.min(6));
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            if self.health_check().await.unwrap_or(false) {
                return Ok(());
            }
        }

        Err(TextRuntimeError::PandocHealthCheckTimeout(10_000))
    }

    /// Health check: GET http://127.0.0.1:{port}/version → expect 200 OK.
    ///
    /// Returns `Ok(true)` if the server responds with a success status code.
    /// Returns `Ok(false)` if the request fails (connection refused, timeout, etc.).
    pub async fn health_check(&self) -> Result<bool, TextRuntimeError> {
        let url = format!("http://127.0.0.1:{}/version", self.config.pandoc_port);
        match self.client.get(&url).send().await {
            Ok(resp) => Ok(resp.status().is_success()),
            Err(_) => Ok(false),
        }
    }

    /// Convert text from one format to Pandoc JSON AST via pandoc-server.
    ///
    /// POST to http://127.0.0.1:{port}/ with JSON body:
    /// `{"text": "...", "from": format, "to": "json"}`
    ///
    /// Client timeout: 30 seconds.
    /// Returns the raw response body string (pandoc JSON AST).
    pub async fn convert(&self, text: &str, from_format: &str) -> Result<String, TextRuntimeError> {
        let url = format!("http://127.0.0.1:{}/", self.config.pandoc_port);
        let body = serde_json::json!({
            "text": text,
            "from": from_format,
            "to": "json"
        });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    TextRuntimeError::PandocTimeout(30_000)
                } else {
                    TextRuntimeError::PandocServerNotRunning(self.config.pandoc_port)
                }
            })?;

        let response_text = resp.text().await.map_err(|e| {
            TextRuntimeError::InternalError(format!(
                "failed to read pandoc-server response body: {e}"
            ))
        })?;

        Ok(response_text)
    }

    /// Batch convert: POST to http://127.0.0.1:{port}/batch
    ///
    /// Body is a JSON array of {"text":..., "from":..., "to":"json"} objects.
    /// Returns a Vec of results — each element is either the JSON string
    /// (on success) or an error string (on failure). Each element is
    /// independent — one failure doesn't affect others.
    ///
    /// Pandoc-server /batch returns:
    /// `[{"output": "...", "base64": false}, {"error": "..."}]`
    pub async fn convert_batch(
        &self,
        items: &[(String, String)],
    ) -> Result<Vec<Result<String, String>>, TextRuntimeError> {
        let url = format!("http://127.0.0.1:{}/batch", self.config.pandoc_port);

        let batch_items: Vec<serde_json::Value> = items
            .iter()
            .map(|(text, from)| serde_json::json!({"text": text, "from": from, "to": "json"}))
            .collect();

        let resp = self
            .client
            .post(&url)
            .json(&batch_items)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    TextRuntimeError::PandocTimeout(30_000)
                } else {
                    TextRuntimeError::PandocServerNotRunning(self.config.pandoc_port)
                }
            })?;

        let results: Vec<serde_json::Value> =
            resp.json()
                .await
                .map_err(|e| TextRuntimeError::PandocConversionError {
                    doc: "batch".to_string(),
                    message: e.to_string(),
                })?;

        let mut out = Vec::with_capacity(results.len());
        for r in results {
            if let Some(output) = r.get("output").and_then(|v| v.as_str()) {
                out.push(Ok(output.to_string()));
            } else if let Some(error) = r.get("error").and_then(|v| v.as_str()) {
                out.push(Err(error.to_string()));
            } else {
                out.push(Err("unknown response format".to_string()));
            }
        }

        Ok(out)
    }

    /// Convert a file on disk directly using the local pandoc CLI.
    ///
    /// This bypasses pandoc-server and is used for binary formats (like `.docx` or `.epub`)
    /// that cannot easily be sent as raw UTF-8 strings.
    pub async fn convert_file(
        &self,
        file_path: &std::path::Path,
        format: &str,
    ) -> Result<String, TextRuntimeError> {
        let output = tokio::process::Command::new("pandoc")
            .arg(file_path)
            .arg("-f")
            .arg(format)
            .arg("-t")
            .arg("json")
            .output()
            .await
            .map_err(|e| {
                TextRuntimeError::InternalError(format!("Failed to execute pandoc CLI: {e}"))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(TextRuntimeError::PandocConversionError {
                doc: file_path.to_string_lossy().into_owned(),
                message: stderr.into_owned(),
            });
        }

        let json_str = String::from_utf8(output.stdout).map_err(|e| {
            TextRuntimeError::InternalError(format!("Pandoc CLI output is not valid UTF-8: {e}"))
        })?;

        Ok(json_str)
    }

    /// Shutdown pandoc-server gracefully.
    ///
    /// Sends SIGTERM, waits 2 seconds, then sends SIGKILL if the process
    /// hasn't exited. Returns `Ok(())` if the process was already gone.
    pub async fn shutdown(&mut self) -> Result<(), TextRuntimeError> {
        let Some(mut child) = self.process.take() else {
            // Process already gone — nothing to shut down.
            return Ok(());
        };

        // Send SIGTERM.
        let _ = child.start_kill();

        // Wait up to 2 seconds for clean exit.
        match tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => Err(TextRuntimeError::InternalError(format!(
                "error waiting for pandoc-server shutdown: {e}"
            ))),
            Err(_) => {
                // Timeout — force SIGKILL. (tokio >=1.52: Child::kill() is async —
                // must be awaited, otherwise the future is dropped and no signal
                // is sent, hanging shutdown on a wedged pandoc-server forever.)
                let _ = child.kill().await;
                let _ = child.wait().await;
                Ok(())
            }
        }
    }
}

impl Drop for PandocManager {
    /// Best-effort shutdown on drop.
    ///
    /// Sends SIGTERM, checks immediately, then sends SIGKILL if still running.
    /// The `kill_on_drop` flag on the child provides an additional safety net.
    fn drop(&mut self) {
        if let Some(mut child) = self.process.take() {
            // Sync-only context: start_kill() is the sync variant of kill().
            // (tokio >=1.52 made Child::kill() async — calling it here without
            // awaiting would silently drop the SIGKILL.)
            let _ = child.start_kill();
            if let Ok(Some(_)) = child.try_wait() {
                return;
            }
            let _ = child.start_kill();
            let _ = child.try_wait();
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_does_not_spawn() {
        let config = RuntimeConfig {
            pandoc_port: 8472,
            ..Default::default()
        };
        let manager = PandocManager::new(config);
        assert!(manager.process.is_none());
    }

    #[test]
    fn test_health_check_returns_false_when_no_server() {
        // Use a fresh ephemeral port — a fixed port (8472) would race with
        // other tests' pandoc-server instances in the same parallel run.
        let config = RuntimeConfig {
            pandoc_port: crate::test_util::free_port(),
            ..Default::default()
        };
        let manager = PandocManager::new(config);

        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let healthy = rt.block_on(manager.health_check());
        assert!(healthy.is_ok());
        assert!(
            !healthy.unwrap(),
            "health_check should return false when no server is running"
        );
    }

    #[test]
    fn test_new_has_no_process() {
        let config = RuntimeConfig::default();
        let mgr = PandocManager::new(config);
        assert!(mgr.process.is_none());
    }
}
