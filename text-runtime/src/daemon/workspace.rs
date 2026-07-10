// ── WorkspaceHandle ────────────────────────────────────────────────────────
//
// A live workspace: owns a Runtime (behind tokio::sync::Mutex for async-safe
// exclusive access) and a CancellationToken that stops the file watcher when
// the workspace is removed.
//
// Lock discipline:
//   1. Clone Arc<WorkspaceHandle> out of DashMap (drop map ref immediately)
//   2. Lock handle.runtime via Mutex
//   3. Do operation
//   4. Drop Mutex guard
//   Never hold two workspace Mutexes simultaneously.
//   Never hold a DashMap reference across .await.

use std::path::PathBuf;

use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::error::TextRuntimeError;
use crate::runtime::Runtime;

// ── WorkspaceHandle ───────────────────────────────────────────────────────────

/// An active workspace with its associated runtime and watcher lifecycle.
///
/// `runtime` is behind `tokio::sync::Mutex` because `Runtime::ingest_*` take
/// `&mut self` (they mutate the SQLite store via transactions). The Mutex
/// serialises all operations — reads and writes alike — keeping the
/// implementation simple and correct.
pub struct WorkspaceHandle {
    pub name: String,
    pub root: PathBuf,
    pub data_dir: PathBuf,
    /// The semantic engine runtime. Lock before any operation.
    pub runtime: Mutex<Runtime>,
    /// Cancel this token to stop the file watcher task for this workspace.
    pub watcher_token: CancellationToken,
    /// JoinHandle for the watcher background task (if one was spawned).
    pub watcher_task: Mutex<Option<JoinHandle<()>>>,
}

impl WorkspaceHandle {
    /// Open a workspace: create `data_dir/.textruntime/`, open Runtime, return handle.
    ///
    /// The `watcher_token` is fresh; call `spawn_watcher` after this to start
    /// watching. The `watcher_task` is initially None.
    pub async fn open(
        name: String,
        root: PathBuf,
        data_dir: PathBuf,
    ) -> Result<Self, TextRuntimeError> {
        // Create data_dir/.textruntime/ if needed
        let runtime_dir = data_dir.join(".textruntime");
        std::fs::create_dir_all(&runtime_dir).map_err(|e| TextRuntimeError::io(&runtime_dir, e))?;

        let runtime = Runtime::open(&runtime_dir).await?;

        Ok(Self {
            name,
            root,
            data_dir,
            runtime: Mutex::new(runtime),
            watcher_token: CancellationToken::new(),
            watcher_task: Mutex::new(None),
        })
    }

    /// Graceful workspace shutdown:
    ///
    /// 1. Cancel watcher token (stops the file watcher task)
    /// 2. Await watcher task completion
    /// 3. Close Runtime (shuts down pandoc-server, closes DB + WAL checkpoint)
    pub async fn shutdown(self) -> Result<(), TextRuntimeError> {
        self.watcher_token.cancel();

        // Await watcher task if one was spawned
        if let Some(task) = self.watcher_task.into_inner() {
            let _ = task.await;
        }

        // Consume the Mutex to get the Runtime by value, then close it
        let runtime = self.runtime.into_inner();
        runtime.close().await
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_workspace_open_close() {
        let (_tmp, runtime_dir) = crate::test_util::runtime_dir_with_free_port();
        let data_dir = runtime_dir.parent().unwrap().to_path_buf();
        let root = data_dir.join("notes");

        let handle = WorkspaceHandle::open("notes".to_string(), root.clone(), data_dir.clone())
            .await
            .unwrap();

        // Verify the .textruntime directory was created
        assert!(data_dir.join(".textruntime").exists());
        assert!(data_dir.join(".textruntime").join("db.sqlite").exists());
        assert_eq!(handle.name, "notes");

        // Shutdown should complete without error
        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_workspace_cancel_stops_watcher() {
        let (_tmp, runtime_dir) = crate::test_util::runtime_dir_with_free_port();
        let data_dir = runtime_dir.parent().unwrap().to_path_buf();
        let handle = WorkspaceHandle::open("test".to_string(), data_dir.join("root"), data_dir)
            .await
            .unwrap();

        let token = handle.watcher_token.clone();

        // Spawn a task that waits for cancellation
        let task = tokio::spawn(async move {
            token.cancelled().await;
        });

        // Store the task
        *handle.watcher_task.lock().await = Some(task);

        // Shutdown cancels the token and awaits the task
        handle.shutdown().await.unwrap();
        // If we get here, the task completed — cancellation worked
    }
}
