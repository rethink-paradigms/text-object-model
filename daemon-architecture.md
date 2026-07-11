# Daemon Architecture — Text Runtime Phase 2

**Date:** 2026-07-10 (Updated: 2026-07-11)
**Status:** Implemented (Phase 2 Daemon)
**Scope:** Full daemon module design, integration points, locking strategy, lifecycle

> **Implementation note:** The daemon architecture described below has been fully implemented and verified. All 10 daemon modules were built as specified. Key deviations and discoveries during implementation are noted at the end of this document ([§6. Implementation Notes](#6-implementation-notes)).

---

## 1. Module Map

Each file in `src/daemon/` and its responsibility:

```
src/daemon/
├── mod.rs              — DaemonHandle, pub async fn run(config: DaemonConfig)
├── config.rs           — DaemonConfig, WorkspaceConfig, load_config(), ArcSwap wrapper
├── socket.rs           — UnixListener bind, UnixSocketGuard (RAII unlink), accept loop
├── protocol.rs         — Request, Response, Cmd enum, NDJSON read/write helpers
├── handler.rs          — dispatch(cmd, workspace_registry) → Response
├── workspace.rs        — WorkspaceHandle (Runtime + DB + watcher token)
├── registry.rs         — WorkspaceRegistry wrapping Arc<DashMap<String, WorkspaceHandle>>
├── watcher.rs          — Per-workspace FileWatcher, notify→tokio bridge, debounce + change detection
├── lifecycle.rs        — signal setup, tokio::select! loop, graceful shutdown drain
└── lock.rs             — SingleInstance: abstract socket (Linux) / flock on PID file (macOS)
```

### Dependencies between files

```
lock.rs          → no internal deps
config.rs        → no internal deps
protocol.rs      → no internal deps
workspace.rs     → depends on: Runtime (existing), Store (existing)
watcher.rs       → depends on: workspace.rs (WorkspaceHandle)
registry.rs      → depends on: workspace.rs (WorkspaceHandle)
socket.rs        → depends on: protocol.rs, handler.rs, registry.rs
handler.rs       → depends on: protocol.rs, registry.rs, workspace.rs
lifecycle.rs     → depends on: socket.rs, registry.rs, watcher.rs, lock.rs
mod.rs           → depends on: config.rs, lifecycle.rs, lock.rs
```

---

## 2. Struct Definitions

### 2.1 `daemon/config.rs`

```rust
use std::path::PathBuf;
use arc_swap::ArcSwap;
use std::sync::Arc;

/// Per-workspace configuration from config.toml
#[derive(Debug, Clone, serde::Deserialize)]
pub struct WorkspaceConfig {
    pub name: String,
    pub root: PathBuf,                  // ~/notes
    pub data_dir: PathBuf,              // ~/.local/share/text-runtime/notes
    #[serde(default)]
    pub watch_dirs: Vec<PathBuf>,       // ["~/notes/docs"]
}

/// Top-level daemon config from ~/.config/text-runtime/config.toml
#[derive(Debug, Clone, serde::Deserialize)]
pub struct DaemonConfig {
    #[serde(default = "default_socket_path")]
    pub socket_path: PathBuf,           // ~/.local/state/text-runtime/runtime.sock
    #[serde(default = "default_pid_path")]
    pub pid_path: PathBuf,              // ~/.local/state/text-runtime/runtime.pid
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_seconds: u64,      // 1800
    #[serde(default = "default_shutdown_grace")]
    pub shutdown_grace_seconds: u64,    // 10
    pub workspaces: Vec<WorkspaceConfig>,
}

/// Thread-safe config holder. Existing connections keep old Arc;
/// new connections see the new config after SIGHUP reload.
pub struct ConfigHandle {
    inner: ArcSwap<DaemonConfig>,
}

impl ConfigHandle {
    pub fn new(config: DaemonConfig) -> Self {
        Self { inner: ArcSwap::from_pointee(config) }
    }

    pub fn load(&self) -> Arc<DaemonConfig> {
        self.inner.load_full()
    }

    pub fn store(&self, config: DaemonConfig) {
        self.inner.store(Arc::new(config));
    }
}

/// Load config from ~/.config/text-runtime/config.toml,
/// expanding ~ and ${XDG_*} variables.
pub fn load_config(path: Option<&Path>) -> Result<DaemonConfig, TextRuntimeError>;
```

### 2.2 `daemon/protocol.rs`

```rust
use serde::{Deserialize, Serialize};

/// Client → server request frame.
/// Wire format: `{json}\n` (one NDJSON line)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    /// Client-assigned UUID for request/response correlation.
    pub id: String,                     // UUID v7 string
    /// Command name — parsed into Cmd enum by handler.
    pub cmd: String,
    /// Command-specific parameters (flattened into this object).
    #[serde(default)]
    pub params: serde_json::Value,
}

/// Server → client response frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,           // error code: "WORKSPACE_NOT_FOUND", etc.
}

/// All daemon commands (protocol v1).
/// Deserialized from Request.cmd + Request.params.
#[derive(Debug, Clone)]
pub enum Cmd {
    WorkspaceList,
    WorkspaceAdd {
        name: String,
        root: PathBuf,
        data_dir: PathBuf,
        watch_dirs: Vec<PathBuf>,
    },
    WorkspaceRemove {
        name: String,
    },
    Ingest {
        workspace: String,
        path: PathBuf,
    },
    IngestText {
        workspace: String,
        text: String,
        format: String,
        title: Option<String>,
    },
    Read {
        workspace: String,
        doc_id: String,
        format: Option<String>,
        markers: bool,
    },
    Annotate {
        workspace: String,
        doc_id: String,
        /// UUID of the target sentence — from the marker_map returned by a prior Read call.
        /// Clients resolve §N → UUID themselves using the Read response's marker_map.
        /// §N numbers are session-local and ephemeral; the UUID is stable.
        sentence_uuid: String,
        quote: Option<String>,
        body: Option<String>,
        motivation: Option<String>,
    },
    Search {
        workspace: String,
        query: String,
        doc_id: Option<String>,
    },
    Toc {
        workspace: String,
        doc_id: String,
    },
    Status,
    Shutdown,
}

/// NDJSON helpers: read one line, write one line, with byte cap.
pub const MAX_LINE_BYTES: usize = 1024 * 1024; // 1 MiB

/// Serialize a value to NDJSON line (JSON + \n).
pub fn encode_line<T: Serialize>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    let mut buf = serde_json::to_vec(value)?;
    buf.push(b'\n');
    Ok(buf)
}

/// Parse a Request from a raw NDJSON line.
pub fn parse_request(line: &str) -> Result<Request, serde_json::Error> {
    serde_json::from_str(line)
}
```

### 2.3 `daemon/socket.rs`

```rust
use tokio::net::{UnixListener, UnixStream};
use tokio_util::sync::CancellationToken;
use std::path::{Path, PathBuf};

/// RAII guard: ensures the socket file is unlinked on drop.
/// Handles panic, signal, and normal exit uniformly.
pub struct UnixSocketGuard {
    path: PathBuf,
}

impl UnixSocketGuard {
    pub fn new(path: PathBuf) -> Self {
        // Stale socket cleanup: try unlink before bind.
        // If it fails because nothing's there, that's fine.
        let _ = std::fs::remove_file(&path);
        Self { path }
    }
}

impl Drop for UnixSocketGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Bind a Unix domain socket with tightened permissions (0o600).
/// Returns (listener, guard). The guard ensures cleanup on drop.
pub fn bind_socket(path: &Path) -> io::Result<(UnixListener, UnixSocketGuard)> {
    let guard = UnixSocketGuard::new(path.to_path_buf());

    // Tighten umask to ensure 0o600 permissions
    let prev_umask = unsafe { libc::umask(0o117) };  // 0o660 & ~0o117 = 0o600
    let listener = UnixListener::bind(path);
    unsafe { libc::umask(prev_umask) };

    let listener = listener?;

    // Explicitly set permissions as belt-and-suspenders
    let perms = std::fs::Permissions::from_mode(0o600);
    let _ = std::fs::set_permissions(path, perms);

    Ok((listener, guard))
}

/// Accept loop: spawn a per-connection task for each accepted stream.
/// Runs until `cancel` fires or accept returns a fatal error.
pub async fn accept_loop(
    listener: UnixListener,
    handler: Arc<Handler>,
    cancel: CancellationToken,
    task_tracker: &TaskTracker,
) {
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                // Stop accepting — socket guard will unlink on drop
                return;
            }
            result = listener.accept() => {
                match result {
                    Ok((stream, _addr)) => {
                        let h = handler.clone();
                        let conn_cancel = cancel.child_token();
                        task_tracker.spawn(async move {
                            handle_connection(stream, h, conn_cancel).await;
                        });
                    }
                    Err(e) => {
                        tracing::warn!(?e, "accept error");
                        // Continue accepting — don't crash on transient errors
                        continue;
                    }
                }
            }
        }
    }
}
```

### 2.4 `daemon/handler.rs`

```rust
/// The Handler trait — implemented by the daemon's request dispatcher.
/// One instance per daemon, shared across all connections via Arc.
#[async_trait]
pub trait Handler: Send + Sync + 'static {
    async fn dispatch(&self, req: Request) -> Response;
}

/// Concrete handler: holds Arc to WorkspaceRegistry.
pub struct DaemonHandler {
    registry: Arc<WorkspaceRegistry>,
    start_time: Instant,
    version: &'static str,
    /// Soft-shutdown token from Lifecycle.
    /// Cmd::Shutdown cancels this, which wakes the lifecycle select! loop.
    shutdown_token: CancellationToken,
}

impl DaemonHandler {
    pub fn new(
        registry: Arc<WorkspaceRegistry>,
        version: &'static str,
        shutdown_token: CancellationToken,
    ) -> Self;

#[async_trait]
impl Handler for DaemonHandler {
    async fn dispatch(&self, req: Request) -> Response {
        // Parse cmd from req.cmd + req.params
        let cmd = match parse_cmd(&req) {
            Ok(c) => c,
            Err(e) => return Response::error(&req.id, "BAD_REQUEST", &e),
        };

        match cmd {
            Cmd::WorkspaceList => {
                let ws = self.registry.list().await;
                Response::ok_data(&req.id, json!({ "workspaces": ws }))
            }
            Cmd::WorkspaceAdd { name, root, data_dir, watch_dirs } => {
                match self.registry.add_workspace(name, root, data_dir, watch_dirs).await {
                    Ok(()) => Response::ok(&req.id),
                    Err(e) => Response::error(&req.id, "WORKSPACE_ERROR", &e.to_string()),
                }
            }
            Cmd::WorkspaceRemove { name } => {
                match self.registry.remove_workspace(&name).await {
                    Ok(()) => Response::ok(&req.id),
                    Err(e) => Response::error(&req.id, "WORKSPACE_NOT_FOUND", &e.to_string()),
                }
            }
            Cmd::Ingest { workspace, path } => {
                let handle = match self.registry.get(&workspace) {
                    Some(h) => h,
                    None => return Response::error(&req.id, "WORKSPACE_NOT_FOUND", &format!("workspace '{}' not found", workspace)),
                };
                // Lock the runtime for this operation
                let mut rt = handle.runtime.lock().await;
                match rt.ingest_file(&path).await {
                    Ok(doc_id) => Response::ok_data(&req.id, json!({ "doc_id": doc_id })),
                    Err(e) => Response::error(&req.id, "INGEST_ERROR", &e.to_string()),
                }
            }
            Cmd::IngestText { workspace, text, format, title } => {
                let handle = match self.registry.get(&workspace) {
                    Some(h) => h,
                    None => return Response::error(&req.id, "WORKSPACE_NOT_FOUND", &format!("workspace '{}' not found", workspace)),
                };
                let mut rt = handle.runtime.lock().await;
                let meta = IngestMetadata { title, source_path: None, language: None };
                match rt.ingest_text(&text, &format, &meta).await {
                    Ok(doc_id) => Response::ok_data(&req.id, json!({ "doc_id": doc_id })),
                    Err(e) => Response::error(&req.id, "INGEST_ERROR", &e.to_string()),
                }
            }
            Cmd::Read { workspace, doc_id, format, markers } => {
                let handle = match self.registry.get(&workspace) {
                    Some(h) => h,
                    None => return Response::error(&req.id, "WORKSPACE_NOT_FOUND", &format!("workspace '{}' not found", workspace)),
                };
                let rt = handle.runtime.lock().await;
                let fmt = format.as_deref().unwrap_or("markdown");
                match rt.read(&doc_id, fmt, markers) {
                    Ok(proj) => Response::ok_data(&req.id, json!({
                        "text": proj.text,
                        "format": proj.format,
                        "marker_map": proj.marker_map,
                    })),
                    Err(e) => Response::error(&req.id, "READ_ERROR", &e.to_string()),
                }
            }
            Cmd::Annotate { workspace, doc_id, sentence_uuid, quote, body, motivation } => {
                let handle = match self.registry.get(&workspace) {
                    Some(h) => h,
                    None => return Response::error(&req.id, "WORKSPACE_NOT_FOUND", &format!("workspace '{}' not found", workspace)),
                };
                let rt = handle.runtime.lock().await;
                match rt.annotate(&doc_id, &sentence_uuid, quote.as_deref(), body.as_deref(), motivation.as_deref()) {
                    Ok(anno_id) => Response::ok_data(&req.id, json!({ "annotation_id": anno_id })),
                    Err(e) => Response::error(&req.id, "ANNOTATE_ERROR", &e.to_string()),
                }
            }
            Cmd::Search { workspace, query, doc_id } => {
                let handle = match self.registry.get(&workspace) {
                    Some(h) => h,
                    None => return Response::error(&req.id, "WORKSPACE_NOT_FOUND", &format!("workspace '{}' not found", workspace)),
                };
                let rt = handle.runtime.lock().await;
                match rt.search(&query, doc_id.as_deref()) {
                    Ok(hits) => Response::ok_data(&req.id, json!({ "hits": hits })),
                    Err(e) => Response::error(&req.id, "SEARCH_ERROR", &e.to_string()),
                }
            }
            Cmd::Toc { workspace, doc_id } => {
                let handle = match self.registry.get(&workspace) {
                    Some(h) => h,
                    None => return Response::error(&req.id, "WORKSPACE_NOT_FOUND", &format!("workspace '{}' not found", workspace)),
                };
                let rt = handle.runtime.lock().await;
                match rt.toc(&doc_id) {
                    Ok(entries) => Response::ok_data(&req.id, json!({ "entries": entries })),
                    Err(e) => Response::error(&req.id, "TOC_ERROR", &e.to_string()),
                }
            }
            Cmd::Status => {
                let workspace_count = self.registry.len();
                let uptime = self.start_time.elapsed().as_secs();
                Response::ok_data(&req.id, json!({
                    "version": self.version,
                    "uptime_secs": uptime,
                    "workspace_count": workspace_count,
                }))
            }
            Cmd::Shutdown => {
                // Cancel the soft-shutdown token — this wakes the lifecycle select! loop.
                // The lifecycle drains in-flight tasks and then exits cleanly.
                self.shutdown_token.cancel();
                Response::ok(&req.id)
            }
        }
    }
}
```

### 2.5 `daemon/workspace.rs`

```rust
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// A live workspace: Runtime + watcher lifecycle token.
///
/// Runtime is behind a tokio::sync::Mutex because its ingest methods
/// require &mut self (they mutate DbStore via transaction()).
/// The Mutex serializes all write operations to the workspace.
/// Reads (search, toc, read) also go through the mutex because
/// Runtime::read()/search()/toc() take &self but access fields
/// that are logically shared (marker_map, marker_map lock).
///
/// NOTE: In a future refactor, DbStore could use a connection pool
/// so that reads don't block behind a Mutex. For now, the Mutex
/// is simple and correct.
pub struct WorkspaceHandle {
    pub name: String,
    pub root: PathBuf,
    pub data_dir: PathBuf,
    /// Runtime behind a tokio Mutex. All operations serialize through this.
    pub runtime: Mutex<Runtime>,
    /// Cancelled when the workspace is being removed —
    /// stops the file watcher and in-flight ingest operations.
    pub watcher_token: CancellationToken,
    /// JoinHandle for the watcher task (if any).
    pub watcher_task: Option<JoinHandle<()>>,
}

impl WorkspaceHandle {
    /// Open a new workspace: create data_dir, open Runtime, return handle.
    pub async fn open(
        name: String,
        root: PathBuf,
        data_dir: PathBuf,
    ) -> Result<Self, TextRuntimeError> {
        // Create data_dir/.textruntime/
        let runtime_dir = data_dir.join(".textruntime");
        let runtime = Runtime::open(&runtime_dir).await?;
        Ok(Self {
            name,
            root,
            data_dir,
            runtime: Mutex::new(runtime),
            watcher_token: CancellationToken::new(),
            watcher_task: None,
        })
    }

    /// Graceful workspace shutdown:
    /// 1. Cancel watcher token
    /// 2. Wait for watcher task to finish
    /// 3. Close Runtime (shuts down pandoc-server, closes DB)
    pub async fn shutdown(self) -> Result<(), TextRuntimeError> {
        self.watcher_token.cancel();
        if let Some(task) = self.watcher_task {
            let _ = task.await;
        }
        let runtime = self.runtime.into_inner();
        runtime.close().await
    }
}
```

### 2.6 `daemon/registry.rs`

```rust
use dashmap::DashMap;

/// Concurrent workspace registry.
///
/// Uses DashMap for shard-level locking — reads don't block writes
/// on other shards. Each entry is an Arc<WorkspaceHandle> so that
/// handlers can clone handles out of the map and drop the map reference
/// before doing any async work.
pub struct WorkspaceRegistry {
    inner: DashMap<String, Arc<WorkspaceHandle>>,
}

impl WorkspaceRegistry {
    pub fn new() -> Self {
        Self { inner: DashMap::new() }
    }

    /// Get a workspace handle. Clones the Arc so the caller can
    /// drop the map reference immediately.
    pub fn get(&self, name: &str) -> Option<Arc<WorkspaceHandle>> {
        self.inner.get(name).map(|r| r.value().clone())
    }

    /// Add a workspace. Returns error if name already exists.
    pub fn insert(&self, handle: WorkspaceHandle) -> Result<(), WorkspaceExists> {
        use dashmap::mapref::entry::Entry;
        match self.inner.entry(handle.name.clone()) {
            Entry::Occupied(_) => Err(WorkspaceExists(handle.name)),
            Entry::Vacant(entry) => {
                entry.insert(Arc::new(handle));
                Ok(())
            }
        }
    }

    /// Remove a workspace. Returns the Arc if found.
    pub fn remove(&self, name: &str) -> Option<Arc<WorkspaceHandle>> {
        self.inner.remove(name).map(|(_, v)| v)
    }

    /// List all workspace names and statuses.
    pub fn list(&self) -> Vec<WorkspaceInfo> {
        self.inner.iter().map(|r| {
            let h = r.value();
            WorkspaceInfo {
                name: h.name.clone(),
                root: h.root.clone(),
                status: "active".to_string(),
            }
        }).collect()
    }

    /// Count of active workspaces.
    pub fn len(&self) -> usize {
        self.inner.len()
    }
}
```

### 2.7 `daemon/watcher.rs`

```rust
/// Per-workspace file watcher using notify + tokio bridge.
/// Borrows heavily from the rune PathReloader lego pattern.
pub struct FileWatcher {
    workspace_name: String,
    /// notify → tokio event bridge
    rx: mpsc::UnboundedReceiver<notify::Result<notify::Event>>,
    /// Debounce timer: reset on each event, fires after 500ms of quiet
    debounce: Sleep,
    /// Pending file updates by path
    pending: HashMap<PathBuf, Update>,
    /// Keeps the watcher alive
    _watcher: notify::RecommendedWatcher,
}

#[derive(Clone, PartialEq)]
enum Update {
    Modified,
    Removed,
}

impl FileWatcher {
    /// Create a new watcher for the given directories.
    pub fn new(
        name: String,
        watch_dirs: &[PathBuf],
        debounce_ms: u64,
    ) -> Result<Self, Box<dyn std::error::Error>>;

    /// Wait for the next batch of debounced events.
    /// Returns (modified_paths, removed_paths).
    pub async fn next_batch(self: Pin<&mut Self>) -> (Vec<PathBuf>, Vec<PathBuf>);
}
```

**Change detection pipeline (per file event):**

```
notify event (Create/Modify/Remove)
    ↓
Debounce (500ms timer, reset on each new event)
    ↓
Batch of paths after quiet period
    ↓
Per path:
    1. stat(path) → (size, mtime)
    2. Compare against in-memory cache
    3. If size+mtime match → SKIP (fast path, ~µs)
    4. SHA-256 hash file contents (streaming, 64KB chunks)
    5. Compare hash against stored import_hash in documents table
    6. If hash matches → SKIP
    7. Trigger re-ingest: runtime.ingest_file(path)
    8. Update in-memory cache
```

### 2.8 `daemon/lifecycle.rs`

```rust
use tokio::signal::unix::{SignalKind, signal};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// Central lifecycle manager: signal handling, config reload, graceful shutdown.
///
/// Adapted from element-hq/matrix-authentication-service LifecycleManager lego.
pub struct Lifecycle {
    hard_shutdown: CancellationToken,
    soft_shutdown: CancellationToken,
    tasks: TaskTracker,
    sighup: tokio::signal::unix::Signal,
    sigterm: tokio::signal::unix::Signal,
    sigint: tokio::signal::unix::Signal,
    shutdown_grace: Duration,
}

impl Lifecycle {
    pub fn new(shutdown_grace_seconds: u64) -> Result<Self, io::Error> {
        let hard = CancellationToken::new();
        let soft = hard.child_token();
        Ok(Self {
            hard_shutdown: hard,
            soft_shutdown: soft,
            tasks: TaskTracker::new(),
            sighup: signal(SignalKind::hangup())?,
            sigterm: signal(SignalKind::terminate())?,
            sigint: signal(SignalKind::interrupt())?,
            shutdown_grace: Duration::from_secs(shutdown_grace_seconds),
        })
    }

    pub fn soft_token(&self) -> CancellationToken { self.soft_shutdown.clone() }
    pub fn hard_token(&self) -> CancellationToken { self.hard_shutdown.clone() }
    pub fn tracker(&self) -> &TaskTracker { &self.tasks }

    /// Run the main event loop. Returns on shutdown.
    ///
    /// - SIGTERM/SIGINT → graceful shutdown
    /// - SIGHUP → calls on_reload callback
    pub async fn run<F>(&mut self, mut on_reload: F)
    where
        F: FnMut() -> BoxFuture<'static, ()>,
    {
        loop {
            tokio::select! {
                () = self.soft_shutdown.cancelled() => break,
                _ = self.sigterm.recv() => {
                    tracing::info!("SIGTERM — shutting down");
                    break;
                }
                _ = self.sigint.recv() => {
                    tracing::info!("SIGINT — shutting down");
                    break;
                }
                _ = self.sighup.recv() => {
                    tracing::info!("SIGHUP — reloading config");
                    on_reload().await;
                }
            }
        }

        // Soft shutdown
        self.soft_shutdown.cancel();
        self.tasks.close();

        // Wait for graceful drain or timeout
        let timeout = tokio::time::sleep(self.shutdown_grace);
        tokio::select! {
            _ = self.sigterm.recv() => tracing::warn!("Second signal — hard shutdown"),
            _ = self.sigint.recv() => tracing::warn!("Second signal — hard shutdown"),
            () = timeout => tracing::warn!("Shutdown grace period expired"),
            () = self.tasks.wait() => {}, // happy path
        }

        self.hard_shutdown.cancel();
    }
}
```

### 2.9 `daemon/lock.rs`

```rust
/// Single-instance enforcement.
///
/// Linux: abstract Unix socket bind (EADDRINUSE = already running).
/// macOS: flock on PID file.
///
/// Holds the lock for the daemon's lifetime. Drop releases it.
pub struct DaemonLock {
    #[cfg(target_os = "linux")]
    _sock: Option<OwnedFd>,
    #[cfg(target_os = "macos")]
    _file: File,
    #[cfg(target_os = "macos")]
    is_single: bool,
    pid_file: PathBuf,
}

impl DaemonLock {
    pub fn acquire(runtime_dir: &Path, app_name: &str) -> Result<Self, DaemonLockError>;

    /// Returns true if this is the only instance.
    pub fn is_single(&self) -> bool;

    /// Write PID + get exclusive flock.
    fn write_pid_file(path: &Path) -> io::Result<File>;
}
```

### 2.10 `daemon/mod.rs`

```rust
/// Top-level daemon entry point.
///
/// `text-runtime daemon` → this function.
pub async fn run(config: DaemonConfig) -> Result<(), Box<dyn std::error::Error>> {
    // 1. Acquire single-instance lock
    let lock = DaemonLock::acquire(&config.pid_path.parent().unwrap(), "text-runtime")?;
    if !lock.is_single() {
        eprintln!("daemon already running (PID file locked)");
        std::process::exit(1);
    }

    // 2. Wrap config in ConfigHandle (ArcSwap for hot reload)
    let config_handle = ConfigHandle::new(config);

    // 3. Open all workspaces
    let registry = Arc::new(WorkspaceRegistry::new());
    for ws_cfg in config_handle.load().workspaces.iter() {
        let handle = WorkspaceHandle::open(
            ws_cfg.name.clone(),
            ws_cfg.root.clone(),
            ws_cfg.data_dir.clone(),
        ).await?;
        registry.insert(handle)?;
    }

    // 4. Set up lifecycle (signals)
    let mut lifecycle = Lifecycle::new(config_handle.load().shutdown_grace_seconds)?;

    // 5. Bind socket + start accept loop
    let (listener, _guard) = bind_socket(&config_handle.load().socket_path)?;
    let handler = Arc::new(DaemonHandler::new(registry.clone(), env!("CARGO_PKG_VERSION"), lifecycle.soft_token()));
    let cancel = lifecycle.soft_token();
    lifecycle.tracker().spawn(async move {
        accept_loop(listener, handler, cancel, lifecycle.tracker()).await;
    });

    // 6. Start file watchers for each workspace
    // (spawned as tracked tasks, cancelled via ws.watcher_token)

    // 7. Register SIGHUP reload
    let reg = registry.clone();
    let cfg = config_handle.clone();
    lifecycle.run(move || {
        let reg = reg.clone();
        let cfg = cfg.clone();
        Box::pin(async move {
            reload_config(&cfg, &reg).await;
        })
    }).await;

    // 8. Cleanup: drain connections, shutdown workspaces, unlink socket
    drop(lock); // releases PID file lock
    Ok(())
}
```

---

## 3. Locking Strategy

```
┌─────────────────────────────────────────────────────────────────┐
│                    LOCKING HIERARCHY                             │
│                                                                 │
│  DaemonLock (PID flock / abstract socket)                       │
│  └── held for daemon lifetime, drop on exit                     │
│                                                                 │
│  ConfigHandle (ArcSwap<DaemonConfig>)                           │
│  └── lock-free reads, atomic swap on SIGHUP                     │
│                                                                 │
│  WorkspaceRegistry (DashMap<String, Arc<WorkspaceHandle>>)      │
│  └── shard-level locking, O(1) reads without blocking writers   │
│      └── WorkspaceHandle                                        │
│          └── runtime: Mutex<Runtime>   ← serializes all ops      │
│          └── watcher_token: CancellationToken                   │
│                                                                 │
│  DbStore.conn (rusqlite::Connection)                            │
│  └── protected by Mutex<Runtime> (not directly exported)       │
│  └── SQLite WAL mode: concurrent reads across connections ok,  │
│      but we have 1 conn == all go through Mutex                 │
└─────────────────────────────────────────────────────────────────┘
```

**Lock order discipline:**

1. Clone `Arc<WorkspaceHandle>` out of `DashMap` (drop map ref immediately)
2. Lock `handle.runtime` via `Mutex`
3. Do operation
4. Drop `Mutex` guard

Never hold two workspace mutexes simultaneously. Never hold a map reference across `.await`.

---

## 4. Lifecycle Sequences

### 4.1 Startup

```
1. Parse CLI: text-runtime daemon
2. Load config from ~/.config/text-runtime/config.toml
3. Acquire DaemonLock (single-instance check)
   ├── Linux: bind abstract socket → EADDRINUSE == already running
   └── macOS: flock PID file → EWOULDBLOCK == already running
4. Write PID file atomically (create_new(true))
5. For each workspace in config:
   ├── Create data_dir/.textruntime/ if needed
   ├── Runtime::open(runtime_dir) → Store, PandocManager
   ├── Wrap in WorkspaceHandle { runtime: Mutex<Runtime>, watcher_token }
   └── Insert into WorkspaceRegistry
6. Create Lifecycle manager (install SIGTERM, SIGINT, SIGHUP handlers)
7. Bind Unix socket (umask tighten → bind → chmod 0o600 → UnixSocketGuard)
8. Spawn accept loop (tracked by TaskTracker)
9. Spawn per-workspace file watchers (tracked by TaskTracker)
10. Enter lifecycle.run() — blocks until shutdown signal
```

### 4.2 Shutdown

```
1. SIGTERM or SIGINT received
2. soft_shutdown_token.cancel()
3. Accept loop: stops accepting new connections (cancel fires in select!)
4. TaskTracker.close() — no new tasks
5. Wait for in-flight connections to drain (up to shutdown_grace_seconds)
   ├── Each connection: either completes naturally or sees cancel
   └── Per-connection child tokens ensure clean exit
6. On timeout or second signal: hard_shutdown.cancel()
7. TaskTracker.wait() — all tasks must complete
8. For each workspace in registry:
   ├── Cancel watcher_token → stops file watcher
   ├── Await watcher_task join
   ├── Runtime.close() → PandocManager.shutdown() + Store.close()
   │   ├── PandocManager: SIGTERM pandoc-server, wait 2s, SIGKILL
   │   └── Store.close(): WAL checkpoint, close Connection
   └── Remove from registry
9. Unlink socket file (UnixSocketGuard Drop)
10. Release PID file lock (DaemonLock Drop)
11. Exit 0
```

### 4.3 SIGHUP Config Reload

```
1. SIGHUP received
2. Re-read config.toml from disk
3. Parse and validate (return error if invalid — keep old config)
4. Diff old workspaces vs new:
   ├── Added workspaces:
   │   ├── Create data_dir
   │   ├── Runtime::open()
   │   ├── Wrap in WorkspaceHandle
   │   └── Insert into registry + start watcher
   ├── Removed workspaces:
   │   ├── Remove from registry (returns Arc)
   │   ├── Cancel watcher_token
   │   ├── Await in-flight operations drain
   │   ├── Runtime.close()
   │   └── Drop Arc
   └── Unchanged workspaces: no action
5. ArcSwap::store(new_config)
   ├── Existing IPC connections: hold old Arc (zero impact)
   └── New connections: see new config
6. Log: "Config reloaded: N workspaces, M added, K removed"
```

---

## 5. File Watcher Event Flow

```
┌─────────────────────────────────────────────────────────────────┐
│              FILE WATCHER EVENT FLOW                            │
│                                                                 │
│  notify::recommended_watcher (separate thread)                  │
│      │                                                           │
│      │ Create/Modify/Remove events                               │
│      ▼                                                           │
│  mpsc::unbounded_channel → tokio async                          │
│      │                                                           │
│      │ Per-workspace receiver                                    │
│      ▼                                                           │
│  Debounce (Sleep::reset on each event, 500ms quiet window)     │
│      │                                                           │
│      │ Timer fires after quiet                                   │
│      ▼                                                           │
│  Batch of paths grouped by Update::Modified / Update::Removed   │
│      │                                                           │
│      │ Per path, sequentially:                                   │
│      ▼                                                           │
│  Pre-filter: stat(path) → (size, mtime)                         │
│      │ match cached values → SKIP                                │
│      ▼                                                           │
│  SHA-256 hash file contents (64KB streaming chunks)             │
│      │ match stored import_hash → SKIP                           │
│      ▼                                                           │
│  Lock workspace runtime mutex                                   │
│      │                                                           │
│      ▼                                                           │
│  runtime.ingest_file(path) → re-ingest pipeline                 │
│      │   ├── parse → segment → diff → keep/update/delete nodes  │
│      │   └── activity logged                                    │
│      ▼                                                           │
│  Update in-memory stat cache (size + mtime)                     │
│  Drop runtime mutex                                             │
└─────────────────────────────────────────────────────────────────┘
```

**Cache structure (per workspace, in-memory, not persisted):**

```rust
HashMap<PathBuf, (u64, SystemTime)>  // path → (size, mtime)
```

The import_hash for each document is stored in SQLite (`documents.import_hash`). The stat cache is a fast pre-filter to avoid hashing.

---

## 6. Runtime Integration: Wrapping Strategy

### Problem

`Runtime` has `&mut self` methods (`ingest_file`, `ingest_text`) and `&self` methods (`read`, `search`, `annotate`, `toc`, `transclude`).

The `&mut self` requirement comes from:
- `run_pipeline()` takes `&mut Store`
- `Store::db.transaction()` takes `&mut self` (rusqlite `Connection::transaction()` requires `&mut self`)
- `DbStore` owns `conn: Connection` directly — not behind a Mutex

### Solution

Wrap `Runtime` in a `tokio::sync::Mutex<Runtime>`. All operations (reads and writes) go through the mutex.

**Why `tokio::sync::Mutex` and not `std::sync::Mutex`?**

The ingest operations call `tokio::process::Child` (pandoc-server) and `reqwest` HTTP — both async. Holding a `std::sync::Mutex` across `.await` is a deadlock on the current thread. `tokio::sync::Mutex` is designed for async code.

**Why all operations go through one mutex?**

- Simplicity. Reads are mostly fast (SQLite queries) and won't block long.
- The marker_map Mutex inside Runtime depends on shared state.
- We can optimize later with a read/write split if needed.

**Future optimization path (not now):**

If contention becomes visible, the mutex on Runtime can be split:
- Wrap `Store.db` in a connection pool (one writer + N readers in WAL mode)
- Keep `Runtime.ingest_*` behind a write lock
- Allow `Runtime.read/search/toc/toc` behind a read lock
- `annotate` needs write lock (inserts annotations)

### PandocManager integration

`PandocManager` manages a `tokio::process::Child`. It's owned by `Runtime` (not `Arc`-wrapped). When the Runtime is behind a `Mutex`, the PandocManager is naturally serialized too.

On daemon shutdown:
- `WorkspaceHandle::shutdown()` calls `Runtime::close()`
- `Runtime::close()` drops `self.pandoc` → `PandocManager::Drop` → SIGTERM pandoc-server
- Then `Store::close()` → WAL checkpoint + drop connection

### Store / DbStore thread safety

`DbStore` holds `conn: Connection` (not `Send + Sync` in the traditional sense, but rusqlite `Connection` is `Send`). Wrapped inside `Mutex<Runtime>`, it's safely accessed from one task at a time.

SQLite WAL mode is already enabled (`PRAGMA journal_mode = WAL`). This means:
- Concurrent reads from different connections would not block
- But we have a single connection behind a Mutex, so this doesn't matter yet

---

## 7. Schema Migration Decision

### The question

The existing SQLite schema uses plain table names:
- `documents`, `nodes`, `nodes_fts`, `annotations`, `transclusions`, `activities`

The storage architecture design calls for `engine_` prefixed tables alongside `store_` tables:
- `engine_documents`, `engine_nodes`, `engine_fts`, `engine_transclusions`, `engine_activities`
- `store_annotations`, `store_bookmarks`, `store_agent_notes`, `store_review_state`

### Decision: **Do NOT rename existing tables.**

**Rationale:**

1. **The existing codebase has 141 passing tests** (Cargo.toml doesn't show the count but the test files are extensive). Renaming tables means:
   - Changing every `CREATE TABLE` statement
   - Changing every SQL query in `db.rs` (100+ references to table names)
   - Changing FTS5 triggers (which reference table names)
   - Changing every test that inserts/selects
   - Risk of breaking subtle query behavior

2. **No functional benefit right now.** The `engine_` prefix was motivated by having `store_` tables alongside in the same database. But there are no store tables yet. Adding a prefix to existing tables is mechanical churn with no user-visible benefit.

3. **The cleanest path forward:**
   - **Leave engine tables as-is** (documents, nodes, nodes_fts, annotations, transclusions, activities)
   - **Add new store tables** with `store_` prefix when needed: `store_bookmarks`, `store_agent_notes`, `store_review_state`
   - The naming convention is clear: unprefixed = engine, `store_` = store
   - If we ever need to split into separate databases, the table names already disambiguate

### What if we want `engine_` prefixes later?

At that point, write a migration:
```sql
ALTER TABLE documents RENAME TO engine_documents;
ALTER TABLE nodes RENAME TO engine_nodes;
-- etc.
-- Recreate all FTS triggers to reference new table names
-- Recreate all indexes
```

This would be a one-time migration with a schema version bump. But it is out of scope for Phase 2 and would add risk to an otherwise clean implementation.

---

## 8. New Crates to Add

Add to `Cargo.toml`:

```toml
```toml
# Daemon support (Phase 2)
arc-swap = "1"              # Lock-free config hot-reload
dashmap = "2"               # Concurrent workspace registry
toml = "0.8"                # Config file parsing
tokio-util = { version = "0.7", features = ["rt"] }  # CancellationToken, TaskTracker
nix = { version = "0.29", features = ["signal", "fs"] }  # Abstract socket, signal handling
libc = "0.2"                # umask control
futures-util = "0.3"        # FutureExt, BoxFuture for reload callbacks
dirs = "5"                  # XDG home dir expansion for ~ in config paths
```

---

## 9. Conflicts and Resolutions

### Conflict 1: Runtime `&mut self` vs concurrent daemon access

**Problem:** The daemon receives concurrent IPC requests for the same workspace. `Runtime::ingest_file()` and `ingest_text()` take `&mut self`.

**Resolution:** Wrap `Runtime` in `tokio::sync::Mutex`. All operations serialize through the mutex. For read-heavy workloads this is fine because reads are fast SQLite queries. If profiling shows contention: implement a read/write split (separate writer mutex + reader pool). This is deferred to a future optimization.

### Conflict 2: `DbStore::transaction(&mut self)` vs thread safety

**Problem:** `DbStore::transaction()` takes `&mut self` because `rusqlite::Connection::transaction()` requires `&mut self`. This prevents multiple concurrent transactions.

**Resolution:** This is solved by the Mutex on Runtime. Only one thread holds the Mutex at a time, so only one transaction can run. This is correct (SQLite serializes writes anyway) but may be a bottleneck. A future optimization could split `DbStore` into read-only and read-write connections (both in WAL mode, same file).

### Conflict 3: `Store::close()` takes `self` (by move) — ownership transfer

**Problem:** The daemon registry stores `Arc<WorkspaceHandle>` where `WorkspaceHandle` owns `Runtime` which owns `Store`. On workspace removal, we need to call `Store::close()` which consumes `self`. But `Arc::try_unwrap()` only succeeds if there's exactly one reference. In-flight IPC handlers may hold clones of the `Arc`.

**Resolution (concrete):** The `workspace_remove` IPC command follows this exact sequence:
1. `registry.remove(name)` — atomically removes from DashMap, returns `Option<Arc<WorkspaceHandle>>`. New requests for this workspace now get `WORKSPACE_NOT_FOUND` immediately.
2. Cancel `handle.watcher_token` — stops the file watcher.
3. Drop the registry's own Arc clone (by returning from the remove function). The `ok` response is sent to the client.
4. Any in-flight IPC request holding a clone of the Arc completes normally — it has already cloned the Arc before the remove happened.
5. When the **last** Arc clone drops, `WorkspaceHandle` drops, which drops `tokio::sync::Mutex<Runtime>`, which drops `Runtime`.

**Action needed:** Add `impl Drop for Runtime` that performs sync close — specifically, calls `PandocManager::kill_sync()` (send SIGKILL without awaiting) and lets `rusqlite::Connection`'s own Drop handle the DB close (rusqlite Connection closes on drop, WAL checkpoint happens automatically on close). Full async `Runtime::close()` is used during graceful daemon shutdown (Step 11) where we can `.await`. Drop is only the fallback for the Arc-draining case.

**Recommended:** During daemon exit (lifecycle shutdown), iterate the registry and call `handle.shutdown().await` for each workspace explicitly — this is the clean path. The Drop impl is a safety net for workspace removal mid-run, not the primary shutdown mechanism.
### Conflict 4: Schema naming (plain vs `engine_` prefix)

**Resolution:** Keep existing table names as-is. See Section 7.

### Conflict 5: `annotation.rs` module declared but file missing

In `lib.rs`: `pub mod annotation;` is declared but `src/annotation.rs` does not exist. Instead, there's `src/annotation/mod.rs`, `types.rs`, `reconcile.rs`, `anchoring.rs`. This is a directory module — correct. The declaration `pub mod annotation;` in `lib.rs` resolves to `src/annotation/mod.rs`. No conflict.

### Conflict 6: Naming clash with existing `src/daemon.rs`

The existing `daemon.rs` is a simple watcher. The new module will be `src/daemon/mod.rs` with multiple sub-modules. The old `daemon.rs` would conflict with the new `daemon/` directory. Plan:
- Move the existing watcher logic into `src/daemon/watcher.rs` (adapted)
- Delete the old `src/daemon.rs`
- The new `src/daemon/mod.rs` becomes the daemon module entry point
- Update `lib.rs`: `pub mod daemon;` now resolves to `src/daemon/mod.rs`
- Update `main.rs`: the `cmd_daemon` function should call `daemon::run(config)` instead of `daemon::run_daemon()`

---

## 10. Implementation Notes (Post-Build)

The architecture described above guided the Phase 2 daemon implementation. After building and testing, the following notes document what was actually built and what changed:

### What was built exactly as designed

- **All 10 daemon modules** (`mod.rs`, `config.rs`, `socket.rs`, `protocol.rs`, `handler.rs`, `workspace.rs`, `registry.rs`, `watcher.rs`, `lifecycle.rs`, `lock.rs`) — module map and dependencies match the design.
- **11 commands** in the protocol (`workspace_list`, `workspace_add`, `workspace_remove`, `ingest`, `ingest_text`, `read`, `annotate`, `search`, `toc`, `status`, `shutdown`).
- **Unix socket transport** with NDJSON framing, 1 MiB line cap.
- **Single-instance locking** via `flock` (macOS) / abstract socket (Linux).
- **Graceful lifecycle** with SIGINT/SIGTERM handling, connection draining, and WAL sync.
- **SIGHUP hot-reload** for configuration changes.

### What changed during implementation

1. **UUID-only annotation API**: The original design had the daemon maintain session-local §N → UUID mappings. In the implementation, the `read` endpoint returns a `marker_map` alongside the rendered text. The client resolves §N → UUID and passes the UUID directly to `annotate`. The daemon is stateless with respect to markers. (See `storage-architecture.md §11` for details.)

2. **macOS temp directory symlinks**: `notify` failed to trigger file events in tests because macOS `/var` is a symlink to `/private/var`. Fixed by canonicalizing paths with `fs::canonicalize` in test setup.

3. **Process-global `umask` mutex**: `bind_socket` tightens the umask to `0o117` briefly, which is process-global. Under concurrent testing, this caused `PermissionDenied` errors in sibling threads. Resolved by forcing sequential test execution (`--test-threads=1`) in the daemon test target.

### Verification

The daemon was verified with four heavyweight E2E test scenarios in `tests/e2e_daemon_scenarios.rs`:

- **Scenario 1 (Rapid Research)**: Multiple file writes, concurrent ingest + search queries, annotation creation, re-ingest after edit, verification that sentence UUIDs and annotations remain stable.
- **Scenario 2 (Chaos & Concurrency)**: 5 concurrent clients with batch ingests, searches, status queries, and sudden connection drops. Verifies clean connection isolation and no resource leaks.
- **Scenario 3 (Dynamic Hot-Reload)**: Mock configuration, dynamic workspace addition, SIGHUP reload, verification of new workspace watcher.
- **Scenario 4 (Recovery & Unlink)**: Duplicate daemon rejection via singleton lock, graceful shutdown with socket and PID file cleanup.


