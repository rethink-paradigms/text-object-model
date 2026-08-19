// ── Shared integration-test helpers ─────────────────────────────────────────
//
// Test binaries (tests/*.rs) compile the library without `cfg(test)`, so they
// can't use `crate::test_util` — this module is the integration-test copy.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

use tempfile::TempDir;

/// Ports already handed out by `free_port()` in this test process.
///
/// The naive "bind 127.0.0.1:0, read the port, drop the listener" trick only
/// *probes* availability — the port is released again immediately, so a second
/// concurrent call (from another test running in the same process) can be
/// handed the very same number. When both tests later start a pandoc-server on
/// "their" port, the second bind fails with EADDRINUSE.
///
/// Keeping a process-local registry makes the allocation unique within the
/// binary, which is exactly where the races happen (all scenarios share one
/// test process). Cross-process collisions are not possible in practice: the
/// kernel's ephemeral-port allocator hands each live bind a distinct port, and
/// the range (tens of thousands) dwarfs the handful of ports these tests use.
static USED_PORTS: Mutex<Option<HashSet<u16>>> = Mutex::new(None);

/// Allocate a free TCP port on loopback for a pandoc-server instance.
///
/// Never use a fixed port in tests: parallel test runs (and stray servers left
/// over from crashed runs) would collide. Each test gets its own ephemeral port.
pub fn free_port() -> u16 {
    let mut guard = USED_PORTS.lock().expect("used-ports mutex poisoned");
    let used = guard.get_or_insert_with(HashSet::new);
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

/// Create a fresh `.textruntime/` dir with a config.json pointing at a unique
/// pandoc port, ready for `Runtime::open`.
///
/// Not every test binary uses both helpers; keep this visible to clippy.
#[allow(dead_code)]
pub fn runtime_dir_with_free_port() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().expect("temp dir");
    let runtime_dir = tmp.path().join(".textruntime");
    std::fs::create_dir_all(&runtime_dir).expect("create runtime dir");
    std::fs::write(
        runtime_dir.join("config.json"),
        format!("{{\"pandoc_port\": {}}}\n", free_port()),
    )
    .expect("write config.json");
    (tmp, runtime_dir)
}
