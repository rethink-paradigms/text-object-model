// ── Daemon Module Root ─────────────────────────────────────────────────────
//
// Orchestrates the 9-step daemon startup sequence:
//
//   1. Acquire DaemonLock (single-instance enforcement)
//   2. Wrap config in ConfigHandle (ArcSwap for hot-reload)
//   3. Open all configured workspaces → insert into registry
//   4. Create Lifecycle (signal handlers, TaskTracker)
//   5. Bind Unix socket → UnixSocketGuard (RAII unlink on drop)
//   6. Spawn accept loop (tracked via TaskTracker)
//   7. Spawn per-workspace file watchers (tracked via TaskTracker)
//   8. Register SIGHUP reload handler (diff workspaces, hot add/remove)
//   9. `lifecycle.run()` — blocks until SIGTERM/SIGINT
//  10. After shutdown: drain workspaces, write final log

pub mod config;
pub mod handler;
pub mod lifecycle;
pub mod lock;
pub mod protocol;
pub mod registry;
pub mod socket;
pub mod watcher;
pub mod workspace;

use std::sync::Arc;

use futures_util::future::BoxFuture;

use crate::error::TextRuntimeError;

use self::config::{load_config, ConfigHandle, DaemonConfig};
use self::handler::DaemonHandler;
use self::lifecycle::Lifecycle;
use self::lock::DaemonLock;
use self::registry::WorkspaceRegistry;
use self::socket::{accept_loop, bind_socket};
use self::watcher::spawn_watcher;
use self::workspace::WorkspaceHandle;

/// Start the daemon and run until SIGTERM/SIGINT.
///
/// This is the main entry point called by `main.rs` when the `daemon` subcommand
/// is invoked. It is also the natural entry point for integration tests that
/// start the daemon in-process.
pub async fn run(config: DaemonConfig) -> Result<(), TextRuntimeError> {
    tracing::info!(
        "text-runtime daemon v{} starting",
        env!("CARGO_PKG_VERSION")
    );

    // ── Step 1: Single-instance lock ────────────────────────────────────────

    let state_dir = config
        .socket_path
        .parent()
        .unwrap_or(std::path::Path::new("/tmp"))
        .to_path_buf();

    let lock = DaemonLock::acquire(&state_dir, "text-runtime")?;
    if !lock.is_single() {
        return Err(TextRuntimeError::DaemonAlreadyRunning);
    }
    tracing::info!("single-instance lock acquired");

    // ── Step 2: Config handle (ArcSwap for SIGHUP hot-reload) ───────────────

    let config_handle = Arc::new(ConfigHandle::new(config.clone()));
    let socket_path = config.socket_path.clone();
    let shutdown_grace = config.shutdown_grace_seconds;

    // ── Step 3: Open all configured workspaces ───────────────────────────────

    let registry = Arc::new(WorkspaceRegistry::new());

    for ws_cfg in &config.workspaces {
        tracing::info!("opening workspace '{}'", ws_cfg.name);
        match WorkspaceHandle::open(
            ws_cfg.name.clone(),
            ws_cfg.root.clone(),
            ws_cfg.data_dir.clone(),
        )
        .await
        {
            Ok(handle) => {
                if let Err(e) = registry.insert(handle) {
                    tracing::warn!("duplicate workspace '{}' skipped: {}", ws_cfg.name, e);
                }
            }
            Err(e) => {
                tracing::warn!("failed to open workspace '{}': {}", ws_cfg.name, e);
            }
        }
    }

    tracing::info!("opened {} workspaces", registry.len());

    // ── Step 4: Lifecycle (signal handlers, TaskTracker) ─────────────────────

    let mut lifecycle = Lifecycle::new(shutdown_grace)
        .map_err(|e| TextRuntimeError::DaemonLockError(format!("signal setup failed: {}", e)))?;

    let soft_token = lifecycle.soft_shutdown_token();
    let _hard_token = lifecycle.hard_shutdown_token();
    let tracker = lifecycle.task_tracker().clone();

    // ── Step 5: Bind Unix socket ─────────────────────────────────────────────

    let guard = bind_socket(&socket_path).map_err(|e| {
        TextRuntimeError::SocketError(format!("bind {}: {}", socket_path.display(), e))
    })?;
    let guard = Arc::new(guard);
    tracing::info!("listening on {}", socket_path.display());

    // ── Step 6: Spawn accept loop ─────────────────────────────────────────────

    let handler = Arc::new(DaemonHandler::new(
        Arc::clone(&registry),
        soft_token.clone(),
    ));

    let _accept_handle = tracker.spawn(async move {
        // accept_loop takes ownership of guard arc so socket lives until cancelled
    });

    accept_loop(
        Arc::clone(&guard),
        Arc::clone(&handler),
        soft_token.clone(),
        tracker.clone(),
    );

    // ── Step 7: Spawn file watchers for initial workspaces ────────────────────

    let initial_config = config_handle.load();
    for ws_cfg in &initial_config.workspaces {
        if ws_cfg.watch_dirs.is_empty() {
            continue;
        }
        if let Some(ws_handle) = registry.get(&ws_cfg.name) {
            let cancel = ws_handle.watcher_token.clone();
            let task = spawn_watcher(
                Arc::clone(&ws_handle),
                ws_cfg.watch_dirs.clone(),
                500,
                cancel,
            );
            *ws_handle.watcher_task.lock().await = Some(task);
        }
    }

    // ── Step 8: Register SIGHUP reload handler ────────────────────────────────

    {
        let registry = Arc::clone(&registry);
        let config_handle = Arc::clone(&config_handle);

        lifecycle.register_reload_handler(move || {
            let registry = Arc::clone(&registry);
            let config_handle = Arc::clone(&config_handle);
            Box::pin(async move {
                reload_config(registry, config_handle).await;
            }) as BoxFuture<'static, ()>
        });
    }

    // ── Step 9: Run lifecycle (blocks until SIGTERM/SIGINT) ───────────────────

    tracing::info!("daemon ready");
    let _exit = lifecycle.run().await;

    // ── Post-shutdown: drain workspaces ───────────────────────────────────────

    tracing::info!("shutting down workspaces");
    let names: Vec<String> = registry.list().into_iter().map(|i| i.name).collect();
    for name in names {
        if let Some(arc) = registry.remove(&name) {
            match Arc::try_unwrap(arc) {
                Ok(owned) => {
                    if let Err(e) = owned.shutdown().await {
                        tracing::warn!("error shutting down workspace '{}': {}", name, e);
                    }
                }
                Err(_) => {
                    tracing::warn!(
                        "workspace '{}' still has live references — skipping graceful close",
                        name
                    );
                }
            }
        }
    }

    // Socket unlinks via UnixSocketGuard Drop when `guard` is dropped.
    // Lock releases via DaemonLock Drop when `lock` is dropped.
    drop(guard);
    drop(lock);

    tracing::info!("daemon stopped");
    Ok(())
}

// ── SIGHUP reload ──────────────────────────────────────────────────────────────

/// Hot-reload handler called on SIGHUP.
///
/// Algorithm:
///   1. Re-read config from disk (using same path as startup)
///   2. Diff: new workspace names vs current registry
///   3. Remove workspaces no longer in config
///   4. Add new workspaces from config
///   5. Store new config in ConfigHandle (ArcSwap — new connections see it)
async fn reload_config(registry: Arc<WorkspaceRegistry>, config_handle: Arc<ConfigHandle>) {
    // Re-read config (path was embedded at startup; use None = default path)
    let new_config = match load_config(None) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("SIGHUP reload: failed to read config: {}", e);
            return;
        }
    };

    let new_names: std::collections::HashSet<String> = new_config
        .workspaces
        .iter()
        .map(|w| w.name.clone())
        .collect();
    let current_names: std::collections::HashSet<String> =
        registry.list().into_iter().map(|i| i.name).collect();

    // Remove workspaces no longer in config
    for name in current_names.difference(&new_names) {
        tracing::info!("SIGHUP: removing workspace '{}'", name);
        if let Some(arc) = registry.remove(name) {
            match Arc::try_unwrap(arc) {
                Ok(owned) => {
                    let _ = owned.shutdown().await;
                }
                Err(still_alive) => {
                    still_alive.watcher_token.cancel();
                }
            }
        }
    }

    // Add new workspaces
    for ws_cfg in &new_config.workspaces {
        if current_names.contains(&ws_cfg.name) {
            continue; // already running
        }
        tracing::info!("SIGHUP: adding workspace '{}'", ws_cfg.name);
        match WorkspaceHandle::open(
            ws_cfg.name.clone(),
            ws_cfg.root.clone(),
            ws_cfg.data_dir.clone(),
        )
        .await
        {
            Ok(handle) => {
                if !ws_cfg.watch_dirs.is_empty() {
                    let cancel = handle.watcher_token.clone();
                    if let Err(e) = registry.insert(handle) {
                        tracing::warn!("SIGHUP: workspace insert error: {}", e);
                        continue;
                    }
                    if let Some(ws) = registry.get(&ws_cfg.name) {
                        let task =
                            spawn_watcher(Arc::clone(&ws), ws_cfg.watch_dirs.clone(), 500, cancel);
                        *ws.watcher_task.lock().await = Some(task);
                    }
                } else if let Err(e) = registry.insert(handle) {
                    tracing::warn!("SIGHUP: workspace insert error: {}", e);
                }
            }
            Err(e) => {
                tracing::warn!("SIGHUP: failed to open workspace '{}': {}", ws_cfg.name, e);
            }
        }
    }

    // Swap in the new config — new IPC connections see it immediately
    config_handle.store(new_config);
    tracing::info!("SIGHUP: config reload complete");
}
