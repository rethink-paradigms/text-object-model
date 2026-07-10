// ── WorkspaceRegistry ──────────────────────────────────────────────────────
//
// Concurrent workspace registry backed by DashMap<String, Arc<WorkspaceHandle>>.
// DashMap uses shard-level locking so reads on shard A don't block writes on
// shard B — appropriate for a daemon with many concurrent IPC connections.
//
// Lock order discipline (enforced by API design):
//   - get() clones the Arc out and returns it, so the DashMap reference is
//     dropped immediately. Callers then lock the workspace Mutex separately.
//   - Never hold a DashMap reference across an .await point.

use std::path::PathBuf;
use std::sync::Arc;

use dashmap::DashMap;

use super::workspace::WorkspaceHandle;

// ── Supporting types ──────────────────────────────────────────────────────────

/// Snapshot of a workspace for the `workspace_list` response.
#[derive(Debug, Clone)]
pub struct WorkspaceInfo {
    pub name: String,
    pub root: PathBuf,
    /// Always "active" for now; future: "draining", "error".
    pub status: String,
}

/// Error returned when inserting a workspace that already exists.
#[derive(Debug)]
pub struct WorkspaceExists(pub String);

impl std::fmt::Display for WorkspaceExists {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "workspace '{}' already exists", self.0)
    }
}

// ── WorkspaceRegistry ─────────────────────────────────────────────────────────

/// Concurrent registry of active workspaces.
///
/// All methods are synchronous because DashMap's internal shard locking is
/// not async. Callers must not hold returned DashMap references across `.await`.
pub struct WorkspaceRegistry {
    inner: DashMap<String, Arc<WorkspaceHandle>>,
}

impl WorkspaceRegistry {
    pub fn new() -> Self {
        Self {
            inner: DashMap::new(),
        }
    }

    /// Get a workspace handle by name.
    ///
    /// Clones the `Arc` and returns it immediately — the DashMap reference is
    /// dropped before the function returns, so it is safe to `.await` after.
    pub fn get(&self, name: &str) -> Option<Arc<WorkspaceHandle>> {
        self.inner.get(name).map(|r| r.value().clone())
    }

    /// Insert a workspace. Returns `Err(WorkspaceExists)` if the name is taken.
    pub fn insert(&self, handle: WorkspaceHandle) -> Result<(), WorkspaceExists> {
        use dashmap::mapref::entry::Entry;
        match self.inner.entry(handle.name.clone()) {
            Entry::Occupied(_) => Err(WorkspaceExists(handle.name)),
            Entry::Vacant(e) => {
                e.insert(Arc::new(handle));
                Ok(())
            }
        }
    }

    /// Remove a workspace by name. Returns the `Arc` if found (None if not).
    ///
    /// After removal, new IPC requests for this workspace return WORKSPACE_NOT_FOUND.
    /// In-flight handlers that already cloned the Arc continue operating; the
    /// workspace cleans up when the last Arc drops.
    pub fn remove(&self, name: &str) -> Option<Arc<WorkspaceHandle>> {
        self.inner.remove(name).map(|(_, v)| v)
    }

    /// List all workspaces as `WorkspaceInfo` snapshots.
    pub fn list(&self) -> Vec<WorkspaceInfo> {
        self.inner
            .iter()
            .map(|r| {
                let h = r.value();
                WorkspaceInfo {
                    name: h.name.clone(),
                    root: h.root.clone(),
                    status: "active".to_string(),
                }
            })
            .collect()
    }

    /// Number of active workspaces.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns true if there are no active workspaces.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl Default for WorkspaceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    async fn make_handle(name: &str) -> WorkspaceHandle {
        // Each handle gets its own pandoc-server port — parallel tests must
        // never race on a shared fixed port (see crate::test_util::free_port).
        let (_tmp, runtime_dir) = crate::test_util::runtime_dir_with_free_port();
        let data_dir = runtime_dir.parent().unwrap().to_path_buf();
        WorkspaceHandle::open(name.to_string(), data_dir.join("root"), data_dir)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn test_insert_and_get() {
        let reg = WorkspaceRegistry::new();
        let h = make_handle("notes").await;
        reg.insert(h).unwrap();

        let got = reg.get("notes").unwrap();
        assert_eq!(got.name, "notes");
    }

    #[tokio::test]
    async fn test_insert_duplicate_fails() {
        let reg = WorkspaceRegistry::new();
        let h1 = make_handle("notes").await;
        let h2 = make_handle("notes").await;
        reg.insert(h1).unwrap();
        assert!(reg.insert(h2).is_err());
    }

    #[tokio::test]
    async fn test_remove_and_get_none() {
        let reg = WorkspaceRegistry::new();
        reg.insert(make_handle("work").await).unwrap();

        let removed = reg.remove("work");
        assert!(removed.is_some());
        assert!(reg.get("work").is_none());
    }

    #[tokio::test]
    async fn test_list_returns_all() {
        let reg = WorkspaceRegistry::new();
        reg.insert(make_handle("a").await).unwrap();
        reg.insert(make_handle("b").await).unwrap();
        reg.insert(make_handle("c").await).unwrap();

        let list = reg.list();
        assert_eq!(list.len(), 3);
        let names: Vec<_> = list.iter().map(|i| &i.name).collect();
        assert!(names.iter().any(|n| *n == "a"));
        assert!(names.iter().any(|n| *n == "b"));
        assert!(names.iter().any(|n| *n == "c"));
    }

    #[tokio::test]
    async fn test_remove_returns_handle() {
        let reg = WorkspaceRegistry::new();
        reg.insert(make_handle("blog").await).unwrap();

        let arc = reg.remove("blog").unwrap();
        assert_eq!(arc.name, "blog");
        assert_eq!(reg.len(), 0);
    }

    #[tokio::test]
    async fn test_len_and_is_empty() {
        let reg = WorkspaceRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);

        reg.insert(make_handle("x").await).unwrap();
        assert!(!reg.is_empty());
        assert_eq!(reg.len(), 1);
    }
}
