// ── Unix Socket Server ─────────────────────────────────────────────────────
//
// Adapted from canmi21/vane ndjson-rpc-server.ts lego.
//
// Key design:
//   - UnixSocketGuard: RAII — unlinks socket file on any exit path
//   - bind_socket: stale cleanup → bind → chmod 0o600 (no process-global umask)
//   - handle_connection: biased cancel check → read_line_bounded → dispatch → write
//     Generic over AsyncRead + AsyncWrite for unit testability without sockets
//   - accept_loop: CancellationToken hierarchy — server cancel → per-connection child tokens
//
// All in-flight connections hold child CancellationTokens. When the server
// cancel fires, all connections get their own cancel, ensuring no orphaned tasks.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use super::handler::Handler;
use super::protocol::{encode_line, parse_request, Response, MAX_LINE_BYTES};

// ── UnixSocketGuard ───────────────────────────────────────────────────────────

/// RAII guard that unlinks the socket file when dropped.
///
/// Ensures the socket is cleaned up on any exit path: graceful shutdown,
/// panic, or error. Holds the `UnixListener` alive for its own lifetime.
pub struct UnixSocketGuard {
    pub listener: UnixListener,
    socket_path: PathBuf,
}

impl UnixSocketGuard {
    fn new(listener: UnixListener, socket_path: PathBuf) -> Self {
        Self {
            listener,
            socket_path,
        }
    }
}

impl Drop for UnixSocketGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

// ── bind_socket ───────────────────────────────────────────────────────────────

/// Bind a Unix domain socket at `path`.
///
/// Steps:
///   1. Remove stale socket file (best-effort)
///   2. Bind the socket
///   3. chmod 0o600 (owner read/write only — explicit, not umask-dependent)
///   4. Return `UnixSocketGuard` which unlinks on drop
///
/// Note: we deliberately do NOT manipulate the process umask here. `umask` is
/// process-global state on Unix — changing it from a multithreaded process
/// (the daemon's accept loop runs concurrently with the ingest pipeline) would
/// corrupt the mode of every file/directory created by other threads during
/// the bind window. The explicit `chmod` below gives the socket the exact
/// permissions we want without touching global state.
pub fn bind_socket(path: &Path) -> std::io::Result<UnixSocketGuard> {
    // Remove stale socket if it exists
    let _ = std::fs::remove_file(path);

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let listener = UnixListener::bind(path)?;

    // Explicitly chmod to 0o600 — owner read/write only, no group or world
    if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
        // Don't leak the bound socket file on failure.
        let _ = std::fs::remove_file(path);
        return Err(e);
    }

    Ok(UnixSocketGuard::new(listener, path.to_path_buf()))
}

// ── accept_loop ───────────────────────────────────────────────────────────────

/// Accept connections and spawn per-connection tasks until `cancel` fires.
///
/// Each accepted connection gets a child `CancellationToken` so that a server
/// shutdown cancels all in-flight connections. Tasks are tracked via
/// `TaskTracker` so the server can wait for all connections to drain.
pub fn accept_loop<H: Handler>(
    guard: Arc<UnixSocketGuard>,
    handler: Arc<H>,
    cancel: CancellationToken,
    tracker: TaskTracker,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                // Server shutdown: cancel fires first (biased: checked before accept)
                () = cancel.cancelled() => {
                    tracing::info!("accept loop cancelled — shutting down");
                    return;
                }
                accepted = guard.listener.accept() => {
                    let stream: UnixStream = match accepted {
                        Ok((s, _)) => s,
                        Err(e) => {
                            tracing::warn!("accept() failed: {}", e);
                            continue;
                        }
                    };
                    // Each connection gets a child token — server cancel propagates down
                    let conn_cancel = cancel.child_token();
                    let h = Arc::clone(&handler);
                    tracker.spawn(async move {
                        let (read, write) = stream.into_split();
                        handle_connection(read, write, h, conn_cancel).await;
                    });
                }
            }
        }
    })
}

// ── handle_connection ─────────────────────────────────────────────────────────

/// Per-connection request loop.
///
/// Generic over `R: AsyncRead` and `W: AsyncWrite` so it can be unit-tested
/// with `tokio_test::io::Builder` without a real Unix socket.
///
/// Loop:
///   1. `biased; cancel.cancelled()` → write nothing, return (drain phase)
///   2. `read_line_bounded()` → parse Request → dispatch → encode + write Response
///   3. On EOF or IO error → return cleanly
pub(crate) async fn handle_connection<R, W, H>(
    read: R,
    mut write: W,
    handler: Arc<H>,
    cancel: CancellationToken,
) where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    H: Handler,
{
    let mut reader = BufReader::new(read);
    let mut line = String::new();

    loop {
        let read_result = tokio::select! {
            biased;
            // Cancellation: stop accepting new requests on this connection
            () = cancel.cancelled() => return,
            // Read next NDJSON line (bounded by MAX_LINE_BYTES)
            res = read_line_bounded(&mut reader, &mut line, MAX_LINE_BYTES) => res,
        };

        match read_result {
            // Clean EOF — client disconnected
            Ok(None) => return,
            Ok(Some(())) => {}
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                // Line was too long — send error and close connection
                let resp = Response::error(
                    "0",
                    "LINE_TOO_LONG",
                    &format!("request line exceeded {} bytes", MAX_LINE_BYTES),
                );
                let _ = write_frame(&mut write, &resp).await;
                return;
            }
            Err(e) => {
                tracing::debug!("connection read error: {}", e);
                return;
            }
        }

        if line.is_empty() {
            continue;
        }

        // Parse and dispatch
        let resp = match parse_request(&line) {
            Ok(req) => handler.dispatch(req).await,
            Err(e) => {
                // Cannot echo an id we couldn't parse — use "0"
                Response::error("0", "PARSE_ERROR", &format!("invalid JSON: {}", e))
            }
        };

        if write_frame(&mut write, &resp).await.is_err() {
            return;
        }
    }
}

// ── read_line_bounded ─────────────────────────────────────────────────────────

/// Read one NDJSON line from a buffered reader with a byte-size cap.
///
/// Returns `Ok(Some(()))` when a complete line is available (without `\n`),
/// `Ok(None)` on clean EOF, and `Err(InvalidData)` if the cap is exceeded.
///
/// Adapted from canmi21/vane ndjson-rpc-server.ts.
async fn read_line_bounded<R>(
    reader: &mut BufReader<R>,
    buf: &mut String,
    cap: usize,
) -> std::io::Result<Option<()>>
where
    R: AsyncRead + Unpin,
{
    buf.clear();
    let start_len = buf.len();

    loop {
        let n = reader.read_line(buf).await?;

        if n == 0 {
            // EOF
            return if buf.len() == start_len {
                Ok(None) // Clean EOF with no data
            } else {
                Ok(Some(())) // EOF after partial line — treat as complete
            };
        }

        if buf.ends_with('\n') {
            buf.pop();
            if buf.ends_with('\r') {
                buf.pop();
            }
            if buf.len() > cap {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("line exceeded {}-byte cap", cap),
                ));
            }
            return Ok(Some(()));
        }

        if buf.len() > cap {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("line exceeded {}-byte cap", cap),
            ));
        }
    }
}

// ── write_frame ───────────────────────────────────────────────────────────────

async fn write_frame<W: AsyncWrite + Unpin>(write: &mut W, resp: &Response) -> std::io::Result<()> {
    let bytes = encode_line(resp).map_err(std::io::Error::other)?;
    write.write_all(&bytes).await
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::protocol::Request;
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    // A minimal test handler that echoes requests back as status responses
    struct EchoHandler {
        calls: Mutex<Vec<String>>,
    }

    impl EchoHandler {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(vec![]),
            })
        }
    }

    #[async_trait::async_trait]
    impl Handler for EchoHandler {
        async fn dispatch(&self, req: Request) -> Response {
            self.calls.lock().unwrap().push(req.cmd.clone());
            Response::ok_data(&req.id, json!({ "echo": req.cmd }))
        }
    }

    #[tokio::test]
    async fn test_handle_connection_single_request() {
        use tokio::io::duplex;

        let handler = EchoHandler::new();
        let cancel = CancellationToken::new();

        let req_line = b"{\"id\":\"r1\",\"cmd\":\"status\",\"params\":{}}\n";

        let (client, server) = duplex(1024);
        let (server_read, server_write) = tokio::io::split(server);

        // Write request then EOF
        let (_client_read, mut client_write) = tokio::io::split(client);

        client_write.write_all(req_line).await.unwrap();
        client_write.shutdown().await.unwrap(); // Sends EOF

        handle_connection(server_read, server_write, handler.clone(), cancel).await;

        // Verify the handler was called
        let calls = handler.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], "status");
    }

    #[tokio::test]
    async fn test_handle_connection_cancel_stops_loop() {
        use tokio::io::duplex;

        let handler = EchoHandler::new();
        let cancel = CancellationToken::new();

        let (client, server) = duplex(4096);
        let (server_read, server_write) = tokio::io::split(server);
        let (_, _client_write) = tokio::io::split(client);

        // Cancel immediately
        cancel.cancel();

        // Should return quickly without processing any requests
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            handle_connection(server_read, server_write, handler, cancel),
        )
        .await
        .expect("handle_connection should exit after cancel");
    }

    #[tokio::test]
    async fn test_handle_connection_invalid_json() {
        use tokio::io::duplex;

        let handler = EchoHandler::new();
        let cancel = CancellationToken::new();

        let (client, server) = duplex(4096);
        let (server_read, server_write) = tokio::io::split(server);
        let (mut client_read, mut client_write) = tokio::io::split(client);

        client_write.write_all(b"not valid json\n").await.unwrap();
        client_write.shutdown().await.unwrap();

        let mut output = Vec::new();
        let conn_future = handle_connection(server_read, server_write, handler, cancel);
        let read_future = async {
            use tokio::io::AsyncReadExt;
            client_read.read_to_end(&mut output).await.unwrap();
        };

        tokio::join!(conn_future, read_future);

        let response: Response =
            serde_json::from_slice(&output[..output.len().saturating_sub(1)]).unwrap();
        assert!(!response.ok);
        assert_eq!(response.code.as_deref(), Some("PARSE_ERROR"));
    }

    #[tokio::test]
    async fn test_bind_socket_creates_and_removes() {
        let sock_path =
            std::path::PathBuf::from(format!("/tmp/tr-t1-{}.sock", crate::uuid7::uuid7()));

        {
            let guard = bind_socket(&sock_path).unwrap();
            assert!(sock_path.exists());
            drop(guard);
        }

        // UnixSocketGuard::drop should have removed the file
        assert!(!sock_path.exists());
    }

    #[tokio::test]
    async fn test_bind_socket_permissions() {
        let sock_path =
            std::path::PathBuf::from(format!("/tmp/tr-t2-{}.sock", crate::uuid7::uuid7()));

        let _guard = bind_socket(&sock_path).unwrap();

        let meta = std::fs::metadata(&sock_path).unwrap();
        let mode = meta.permissions().mode();
        // Should be 0o600 (owner read/write only)
        assert_eq!(mode & 0o777, 0o600, "socket should be 0o600");
    }
}
