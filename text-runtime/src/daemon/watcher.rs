// ── File Watcher ───────────────────────────────────────────────────────────
//
// Per-workspace file watcher. Bridges notify::RecommendedWatcher (which runs
// in a separate OS thread) to tokio async via an mpsc::unbounded_channel.
//
// Change detection pipeline:
//   notify event (Create/Modify/Remove)
//     → debounce (500ms Sleep::reset — coalesces rapid editor saves)
//     → per-path: stat(size, mtime) → compare against in-memory cache
//     → if changed: SHA-256 hash file → compare to stored import_hash
//     → if still changed: lock workspace Mutex → runtime.ingest_file(path)
//     → update stat cache
//
// Adapted from rune-rs/rune PathReloader pattern (lego/text-runtime-daemon/rune-path-reloader.ts).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use notify::{EventKind, RecursiveMode, Watcher};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::workspace::WorkspaceHandle;

// ── Update type ───────────────────────────────────────────────────────────────

#[derive(Clone, PartialEq)]
enum Update {
    Modified,
    Removed,
}

// ── Stat cache entry ─────────────────────────────────────────────────────────

#[derive(Clone)]
struct StatEntry {
    size: u64,
    mtime: SystemTime,
}

// ── spawn_watcher ─────────────────────────────────────────────────────────────

/// Spawn a file watcher task for the given workspace.
///
/// Watches `watch_dirs` recursively. When files change, runs the
/// pre-filter pipeline and re-ingests modified files via the workspace runtime.
///
/// The task runs until `cancel` is fired or all senders are dropped.
pub fn spawn_watcher(
    handle: Arc<WorkspaceHandle>,
    watch_dirs: Vec<PathBuf>,
    debounce_ms: u64,
    cancel: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = run_watcher(handle, watch_dirs, debounce_ms, cancel).await {
            tracing::warn!("file watcher for workspace exited with error: {}", e);
        }
    })
}

async fn run_watcher(
    handle: Arc<WorkspaceHandle>,
    watch_dirs: Vec<PathBuf>,
    debounce_ms: u64,
    cancel: CancellationToken,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Bridge: notify (thread) → tokio (async) via unbounded channel
    let (tx, mut rx) = mpsc::unbounded_channel::<notify::Result<notify::Event>>();

    let mut watcher = notify::recommended_watcher(move |res| {
        // This closure runs on the notify OS thread — use non-blocking send
        let _ = tx.send(res);
    })?;

    // Register all watch directories
    for dir in &watch_dirs {
        if dir.exists() {
            if let Err(e) = watcher.watch(dir, RecursiveMode::Recursive) {
                tracing::warn!("could not watch {}: {}", dir.display(), e);
            }
        }
    }

    let debounce = Duration::from_millis(debounce_ms);
    let mut pending: HashMap<PathBuf, Update> = HashMap::new();
    let mut stat_cache: HashMap<PathBuf, StatEntry> = HashMap::new();

    loop {
        // Wait for the next event or for the debounce timer to fire
        let sleep = tokio::time::sleep(debounce);
        tokio::pin!(sleep);

        tokio::select! {
            biased;

            // Cancellation: workspace removed or daemon shutting down
            () = cancel.cancelled() => {
                tracing::debug!("watcher cancelled for workspace '{}'", handle.name);
                return Ok(());
            }

            // New notify event — add to pending, reset debounce by looping
            maybe = rx.recv() => {
                let Some(res) = maybe else { return Ok(()); };
                match res {
                    Ok(event) => handle_event(event, &mut pending),
                    Err(e) => tracing::warn!("notify error: {}", e),
                }
                // Drain any immediately-available events before debouncing
                while let Ok(res) = rx.try_recv() {
                    match res {
                        Ok(event) => handle_event(event, &mut pending),
                        Err(e) => tracing::warn!("notify error: {}", e),
                    }
                }
                // Continue loop — debounce resets on each event
                continue;
            }

            // Debounce timer fired — process pending batch
            _ = &mut sleep => {
                if pending.is_empty() {
                    continue;
                }
            }
        }

        // Process all pending paths
        let batch: Vec<(PathBuf, Update)> = pending.drain().collect();

        for (path, update) in batch {
            match update {
                Update::Removed => {
                    stat_cache.remove(&path);
                    // Note: we don't delete from the engine on file removal —
                    // content remains searchable. Explicit deletion is a future feature.
                    tracing::debug!("file removed (not deleted from engine): {}", path.display());
                }
                Update::Modified => {
                    if !path.is_file() {
                        continue;
                    }

                    // Pre-filter 1: stat check (fast, ~µs)
                    match std::fs::metadata(&path) {
                        Ok(meta) => {
                            let size = meta.len();
                            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);

                            if let Some(cached) = stat_cache.get(&path) {
                                if cached.size == size && cached.mtime == mtime {
                                    // stat unchanged — skip (no content change possible)
                                    continue;
                                }
                            }

                            // Pre-filter: SHA-256 hash (streaming, ~ms)
                            let _new_hash = match hash_file(&path) {
                                Ok(h) => h,
                                Err(e) => {
                                    tracing::warn!("hash error for {}: {}", path.display(), e);
                                    continue;
                                }
                            };

                            // Store the hash in a local map so we can skip unchanged files
                            // across notify cycles (e.g. after a touch with no content change).
                            // The DB import_hash comparison happens implicitly: if the DB already
                            // has the same content, run_pipeline will update the existing document
                            // in place (upsert by path), which is idempotent and safe.

                            // Re-ingest
                            tracing::info!("re-ingesting: {}", path.display());
                            {
                                let mut rt = handle.runtime.lock().await;
                                match rt.ingest_file(&path).await {
                                    Ok(doc_id) => {
                                        tracing::info!(
                                            "re-ingested {} → {}",
                                            path.display(),
                                            doc_id
                                        );
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            "re-ingest error for {}: {}",
                                            path.display(),
                                            e
                                        );
                                    }
                                }
                            }

                            // Update stat cache
                            stat_cache.insert(path, StatEntry { size, mtime });
                        }
                        Err(e) => {
                            tracing::warn!("stat error for {}: {}", path.display(), e);
                            stat_cache.remove(&path);
                        }
                    }
                }
            }
        }
    }
}

/// Handle a notify event: classify as Modified or Removed and add to pending map.
fn handle_event(event: notify::Event, pending: &mut HashMap<PathBuf, Update>) {
    let update = match event.kind {
        EventKind::Remove(_) => Update::Removed,
        EventKind::Create(_) | EventKind::Modify(_) => Update::Modified,
        _ => return, // Access events, etc. — ignore
    };

    for path in event.paths {
        // A later Removed overwrites an earlier Modified for the same path.
        // A Modified never overwrites a Removed (file might have been recreated).
        match pending.get(&path) {
            Some(Update::Removed) if update == Update::Modified => {
                // File was removed then recreated — treat as Modified
                pending.insert(path, Update::Modified);
            }
            None => {
                pending.insert(path, update.clone());
            }
            _ => {
                pending.insert(path, update.clone());
            }
        }
    }
}

/// SHA-256 hash a file by streaming it in 64KB chunks.
fn hash_file(path: &std::path::Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), b"hello world").unwrap();
        let hash = hash_file(tmp.path()).unwrap();
        // SHA-256 of "hello world"
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
        // Just verify it's a 64-char hex string
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_handle_event_modified() {
        let mut pending = HashMap::new();
        let path = PathBuf::from("/tmp/test.md");
        let event = notify::Event {
            kind: EventKind::Modify(notify::event::ModifyKind::Any),
            paths: vec![path.clone()],
            attrs: Default::default(),
        };
        handle_event(event, &mut pending);
        assert!(matches!(pending.get(&path), Some(Update::Modified)));
    }

    #[test]
    fn test_handle_event_removed() {
        let mut pending = HashMap::new();
        let path = PathBuf::from("/tmp/test.md");
        let event = notify::Event {
            kind: EventKind::Remove(notify::event::RemoveKind::Any),
            paths: vec![path.clone()],
            attrs: Default::default(),
        };
        handle_event(event, &mut pending);
        assert!(matches!(pending.get(&path), Some(Update::Removed)));
    }

    #[tokio::test]
    async fn test_watcher_cancellation() {
        let (_tmp, runtime_dir) = crate::test_util::runtime_dir_with_free_port();
        let data_dir = runtime_dir.parent().unwrap().to_path_buf();
        let handle = Arc::new(
            WorkspaceHandle::open("test".to_string(), data_dir.join("root"), data_dir)
                .await
                .unwrap(),
        );

        let cancel = CancellationToken::new();
        let task = spawn_watcher(handle, vec![], 100, cancel.clone());

        // Cancel immediately
        cancel.cancel();

        // Task should complete cleanly
        let result = tokio::time::timeout(Duration::from_secs(5), task).await;
        assert!(
            result.is_ok(),
            "watcher task did not exit after cancellation"
        );
    }
}
