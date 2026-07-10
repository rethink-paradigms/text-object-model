// ── Text Runtime Library ────────────────────────────────────────────────────
//
// Local-first text runtime — ingest, structure, annotate, project.

pub mod cfg;
pub mod error;
pub mod types;
pub mod uuid7;

pub mod pandoc_mgr;
pub mod pipeline;
pub mod reingest;
pub mod store;

pub mod annotation;
pub mod projection;
pub mod runtime;
pub mod transclusion;

pub mod agent;
pub mod daemon;

// ── Test utilities ────────────────────────────────────────────────────────────
//
// Shared helpers for unit tests across modules.

#[cfg(test)]
pub(crate) mod test_util {
    /// Ports already handed out by `free_port()` in this test process.
    ///
    /// `free_port()` binds `127.0.0.1:0` and immediately drops the listener, so
    /// a second concurrent caller can be handed the same port; two tests would
    /// then fight over it when both start a pandoc-server. The registry keeps
    /// allocations unique within the process.
    static USED_PORTS: std::sync::Mutex<Option<std::collections::HashSet<u16>>> =
        std::sync::Mutex::new(None);

    /// Allocate a free TCP port on loopback.
    ///
    /// Binds `127.0.0.1:0`, reads the assigned port, then drops the listener so
    /// pandoc-server can bind it. Used to give each test its own pandoc-server
    /// port — never rely on a fixed port in tests: parallel test runs would all
    /// race to bind the same port and fail.
    pub fn free_port() -> u16 {
        let mut guard = USED_PORTS.lock().expect("used-ports mutex poisoned");
        let used = guard.get_or_insert_with(std::collections::HashSet::new);
        loop {
            let port = std::net::TcpListener::bind("127.0.0.1:0")
                .expect("bind ephemeral port")
                .local_addr()
                .expect("read local addr")
                .port();
            if used.insert(port) {
                return port;
            }
        }
    }

    /// Create a fresh `.textruntime/` dir with a config.json pointing at a
    /// unique pandoc port, ready for `Runtime::open`.
    pub fn runtime_dir_with_free_port() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let runtime_dir = tmp.path().join(".textruntime");
        std::fs::create_dir_all(&runtime_dir).expect("create runtime dir");
        std::fs::write(
            runtime_dir.join("config.json"),
            format!("{{\"pandoc_port\": {}}}\n", free_port()),
        )
        .expect("write config.json");
        (tmp, runtime_dir)
    }
}
