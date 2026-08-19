// ── Daemon Lock — Single-Instance Enforcement ──────────────────────────────
//
// Ensures only one daemon instance runs at a time.
//
// Linux:   bind an abstract Unix domain socket. The name is kernel-managed,
//          no filesystem entry, auto-cleaned when the process exits.
//          EADDRINUSE means another instance already holds the socket.
//
// macOS:   flock(LOCK_EX | LOCK_NB) on a PID file in the state directory.
//          EWOULDBLOCK means another instance holds the lock.
//
// The lock is held for the daemon's lifetime. Dropping DaemonLock releases it.

use std::path::{Path, PathBuf};

use crate::error::TextRuntimeError;

// ── DaemonLock ────────────────────────────────────────────────────────────────

/// Single-instance lock. Held for the daemon's entire lifetime.
///
/// Drop releases the lock (abstract socket fd on Linux, flock on macOS).
/// PID file is written on acquisition and cleaned up on drop.
pub struct DaemonLock {
    pid_file: PathBuf,
    is_single: bool,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    _abstract_sock: Option<std::os::unix::io::OwnedFd>,
    #[cfg(target_os = "macos")]
    _lock_file: Option<std::fs::File>,
}

impl DaemonLock {
    /// Attempt to acquire the single-instance lock.
    ///
    /// Returns `Err(TextRuntimeError::DaemonAlreadyRunning)` if another
    /// instance is already running.
    pub fn acquire(runtime_dir: &Path, app_name: &str) -> Result<Self, TextRuntimeError> {
        // Ensure the state directory exists
        std::fs::create_dir_all(runtime_dir).map_err(|e| {
            TextRuntimeError::DaemonLockError(format!("cannot create state dir: {}", e))
        })?;

        let pid_file = runtime_dir.join(format!("{}.pid", app_name));

        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            Self::acquire_linux(pid_file, app_name)
        }

        #[cfg(target_os = "macos")]
        {
            Self::acquire_macos(pid_file)
        }

        #[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
        {
            // Unsupported platform — no locking, just write PID file
            let _ = write_pid_file(&pid_file);
            Ok(Self {
                pid_file,
                is_single: true,
            })
        }
    }

    /// Returns true if this process is the only daemon instance.
    pub fn is_single(&self) -> bool {
        self.is_single
    }
}

// ── Linux implementation: abstract Unix socket ────────────────────────────────

#[cfg(any(target_os = "linux", target_os = "android"))]
impl DaemonLock {
    fn acquire_linux(pid_file: PathBuf, app_name: &str) -> Result<Self, TextRuntimeError> {
        use nix::sys::socket::{self, AddressFamily, SockFlag, SockType, UnixAddr};

        // Abstract sockets live in the kernel's GLOBAL abstract namespace, so a
        // bare `\0{app_name}` would make EVERY daemon instance on the host
        // (e.g. multiple test daemons, or several deployments) collide — the
        // first binds the name, the rest get EADDRINUSE. Scope the lock to the
        // daemon's state dir so single-instance semantics apply per state dir,
        // not per host.
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (
            app_name,
            pid_file.parent().unwrap_or(std::path::Path::new("/")),
        )
            .hash(&mut hasher);
        let lock_name = format!("{}-{:016x}", app_name, hasher.finish());

        // Build the abstract socket name: \0<lock_name>
        let addr = UnixAddr::new_abstract(lock_name.as_bytes()).map_err(|e| {
            TextRuntimeError::DaemonLockError(format!("abstract socket addr: {}", e))
        })?;

        let sock = socket::socket(
            AddressFamily::Unix,
            SockType::Stream,
            SockFlag::SOCK_CLOEXEC,
            None,
        )
        .map_err(|e| TextRuntimeError::DaemonLockError(format!("socket(): {}", e)))?;

        // nix 0.29: bind takes &dyn SockaddrLike — UnixAddr implements it directly.
        match socket::bind(sock.as_raw_fd(), &addr) {
            Ok(()) => {
                // We are the single instance — write PID file
                let _ = write_pid_file(&pid_file);
                Ok(Self {
                    pid_file,
                    is_single: true,
                    _abstract_sock: Some(sock),
                })
            }
            Err(nix::errno::Errno::EADDRINUSE) => {
                // Another instance holds the socket
                Ok(Self {
                    pid_file,
                    is_single: false,
                    _abstract_sock: None,
                })
            }
            Err(e) => Err(TextRuntimeError::DaemonLockError(format!("bind(): {}", e))),
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
impl Drop for DaemonLock {
    fn drop(&mut self) {
        // Abstract socket: auto-cleaned by kernel when OwnedFd drops.
        // Clean up PID file.
        let _ = std::fs::remove_file(&self.pid_file);
    }
}

// ── macOS implementation: flock on PID file ───────────────────────────────────

#[cfg(target_os = "macos")]
impl DaemonLock {
    fn acquire_macos(pid_file: PathBuf) -> Result<Self, TextRuntimeError> {
        use std::fs::OpenOptions;
        use std::io::Write;
        use std::os::unix::io::AsRawFd;

        // Open or create the PID file
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&pid_file)
            .map_err(|e| TextRuntimeError::DaemonLockError(format!("open pid file: {}", e)))?;

        // Try to acquire an exclusive non-blocking flock
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };

        if rc == 0 {
            // Lock acquired — write our PID
            let _ = file.set_len(0);
            let _ = writeln!(file, "{}", std::process::id());
            Ok(Self {
                pid_file,
                is_single: true,
                _lock_file: Some(file),
            })
        } else {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
                // Another instance holds the lock
                Ok(Self {
                    pid_file,
                    is_single: false,
                    _lock_file: None,
                })
            } else {
                Err(TextRuntimeError::DaemonLockError(format!(
                    "flock(): {}",
                    err
                )))
            }
        }
    }
}

#[cfg(target_os = "macos")]
impl Drop for DaemonLock {
    fn drop(&mut self) {
        // flock is released when the File (_lock_file) is dropped.
        // Clean up PID file.
        if self.is_single {
            let _ = std::fs::remove_file(&self.pid_file);
        }
    }
}

// ── PID file helper ───────────────────────────────────────────────────────────

/// Write our PID to the PID file (best-effort; failures are non-fatal).
///
/// Only used on Linux/Android (abstract-socket lock) and the unsupported-
/// platform fallback; macOS uses flock and never calls this, so allow dead
/// code on macOS builds.
#[allow(dead_code)]
fn write_pid_file(path: &Path) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    writeln!(f, "{}", std::process::id())
}

// ── OwnedFd as_raw_fd shim (Linux) ───────────────────────────────────────────

#[cfg(any(target_os = "linux", target_os = "android"))]
use std::os::unix::io::AsRawFd;

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_acquire_first_instance() {
        let tmp = tempfile::tempdir().unwrap();
        let lock = DaemonLock::acquire(tmp.path(), "test-daemon-first").unwrap();
        assert!(lock.is_single());
    }

    #[test]
    fn test_acquire_second_instance() {
        let tmp = tempfile::tempdir().unwrap();
        let _lock1 = DaemonLock::acquire(tmp.path(), "test-daemon-second").unwrap();
        let lock2 = DaemonLock::acquire(tmp.path(), "test-daemon-second").unwrap();
        // On Linux: second bind returns EADDRINUSE → is_single = false
        // On macOS: second flock returns EWOULDBLOCK → is_single = false
        assert!(!lock2.is_single());
    }

    #[test]
    fn test_drop_releases_lock() {
        let tmp = tempfile::tempdir().unwrap();
        {
            let _lock = DaemonLock::acquire(tmp.path(), "test-daemon-drop").unwrap();
            // lock held here
        }
        // After drop: new acquire should succeed
        let lock2 = DaemonLock::acquire(tmp.path(), "test-daemon-drop").unwrap();
        assert!(lock2.is_single());
    }
}
