// ── Daemon Lifecycle Manager ───────────────────────────────────────────────
//
// Adapted from element-hq/matrix-authentication-service lifecycle.rs lego.
//
// Responsibilities:
//   - Install OS signal handlers (SIGTERM, SIGINT, SIGHUP)
//   - On SIGTERM/SIGINT: soft shutdown (cancel soft token, wait for tasks to drain)
//   - On SIGHUP: call registered reload callbacks (config hot-reload)
//   - After grace period or second signal: hard shutdown (cancel hard token)
//   - Returns ExitCode: SUCCESS (clean) or FAILURE (crash-triggered shutdown)
//
// CancellationToken hierarchy:
//   hard_shutdown (root)
//     └── soft_shutdown (child) ← accept loop + per-conn tasks cancel here

use std::process::ExitCode;
use std::time::Duration;

use futures_util::future::BoxFuture;
use tokio::signal::unix::{Signal, SignalKind};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

// ── Lifecycle ─────────────────────────────────────────────────────────────────

/// Central lifecycle manager.
///
/// Create with `Lifecycle::new()`, register reload handlers, pass the public
/// tokens and task tracker to all subsystems, then call `run()` which blocks
/// until SIGTERM/SIGINT.
pub struct Lifecycle {
    hard_shutdown: CancellationToken,
    soft_shutdown: CancellationToken,
    tasks: TaskTracker,
    sigterm: Signal,
    sigint: Signal,
    sighup: Signal,
    shutdown_grace: Duration,
    reload_handlers: Vec<Box<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>>,
}

impl Lifecycle {
    /// Create a new `Lifecycle`.
    ///
    /// Installs OS signal handlers for SIGTERM, SIGINT, and SIGHUP.
    pub fn new(shutdown_grace_seconds: u64) -> Result<Self, std::io::Error> {
        let hard_shutdown = CancellationToken::new();
        let soft_shutdown = hard_shutdown.child_token();

        let sigterm = tokio::signal::unix::signal(SignalKind::terminate())?;
        let sigint = tokio::signal::unix::signal(SignalKind::interrupt())?;
        let sighup = tokio::signal::unix::signal(SignalKind::hangup())?;

        let tasks = TaskTracker::new();

        Ok(Self {
            hard_shutdown,
            soft_shutdown,
            tasks,
            sigterm,
            sigint,
            sighup,
            shutdown_grace: Duration::from_secs(shutdown_grace_seconds),
            reload_handlers: Vec::new(),
        })
    }

    // ── Public token accessors ────────────────────────────────────────────

    /// The soft shutdown token. Cancel this to drain connections gracefully.
    /// All server tasks (accept loop, per-connection handlers) should select on this.
    pub fn soft_shutdown_token(&self) -> CancellationToken {
        self.soft_shutdown.clone()
    }

    /// The hard shutdown token. Cancel this only after grace period expires.
    /// Very long-running operations can select on this as a last resort.
    pub fn hard_shutdown_token(&self) -> CancellationToken {
        self.hard_shutdown.clone()
    }

    /// The task tracker. Spawn all tasks via `tracker.spawn(...)` so the
    /// lifecycle can wait for them to drain on shutdown.
    pub fn task_tracker(&self) -> &TaskTracker {
        &self.tasks
    }

    // ── Reload registration ───────────────────────────────────────────────

    /// Register a callback to be called on SIGHUP.
    ///
    /// All registered callbacks are awaited concurrently on SIGHUP.
    /// The callback should: re-read config, diff workspaces, hot-add/remove.
    pub fn register_reload_handler<F>(&mut self, handler: F)
    where
        F: Fn() -> BoxFuture<'static, ()> + Send + Sync + 'static,
    {
        self.reload_handlers.push(Box::new(handler));
    }

    // ── run ───────────────────────────────────────────────────────────────

    /// Run the lifecycle: block until SIGTERM or SIGINT, handle SIGHUP reloads.
    ///
    /// Returns `ExitCode::SUCCESS` for clean shutdown, `ExitCode::FAILURE` if
    /// shutdown was triggered by an internal task (likely a crash).
    pub async fn run(mut self) -> ExitCode {
        let crashed = loop {
            tokio::select! {
                // Another task called soft_shutdown.cancel() — treat as crash
                () = self.soft_shutdown.cancelled() => {
                    tracing::warn!("shutdown triggered by internal task");
                    break true;
                }
                _ = self.sigterm.recv() => {
                    tracing::info!("SIGTERM received — graceful shutdown");
                    break false;
                }
                _ = self.sigint.recv() => {
                    tracing::info!("SIGINT received — graceful shutdown");
                    break false;
                }
                _ = self.sighup.recv() => {
                    tracing::info!("SIGHUP received — reloading config");
                    // Run all reload callbacks concurrently
                    let futures: Vec<_> = self.reload_handlers.iter().map(|h| h()).collect();
                    futures_util::future::join_all(futures).await;
                    tracing::info!("Config reload complete");
                    // Stay in the loop — don't shutdown
                }
            }
        };

        // Soft shutdown: stop accepting new connections, drain in-flight
        tracing::info!("initiating soft shutdown");
        self.soft_shutdown.cancel();
        self.tasks.close();

        // Wait for tasks to drain, or timeout, or second signal
        tokio::select! {
            _ = self.sigterm.recv() => {
                tracing::warn!("second SIGTERM — forcing hard shutdown");
            }
            _ = self.sigint.recv() => {
                tracing::warn!("second SIGINT (Ctrl-C) — forcing hard shutdown");
            }
            () = tokio::time::sleep(self.shutdown_grace) => {
                tracing::warn!(
                    "grace period of {}s exceeded — forcing hard shutdown",
                    self.shutdown_grace.as_secs()
                );
            }
            () = self.tasks.wait() => {
                tracing::info!("all tasks drained cleanly");
            }
        }

        // Hard shutdown — cancel everything still running
        self.hard_shutdown.cancel();
        self.tasks.wait().await;

        if crashed {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        }
    }
}
