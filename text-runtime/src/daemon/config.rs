// ── Daemon Configuration ───────────────────────────────────────────────────
//
// Loads and manages daemon configuration from ~/.config/text-runtime/config.toml.
// Uses ArcSwap for lock-free hot-reload on SIGHUP: existing connections hold
// their old Arc; new connections see the updated config after swap.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arc_swap::ArcSwap;
use serde::Deserialize;

use crate::error::TextRuntimeError;

// ── Structs ──────────────────────────────────────────────────────────────────

/// Per-workspace configuration entry from config.toml.
#[derive(Debug, Clone, Deserialize)]
pub struct WorkspaceConfig {
    /// Workspace name (used as key in registry and in IPC commands).
    pub name: String,
    /// Root directory of the workspace (for display/reference).
    pub root: PathBuf,
    /// Directory where text-runtime stores its data (DB, content files).
    /// Defaults to `~/.local/share/text-runtime/<name>`.
    pub data_dir: PathBuf,
    /// Directories to watch for automatic re-ingestion.
    /// Empty means no automatic watching; workspaces are still accessible.
    #[serde(default)]
    pub watch_dirs: Vec<PathBuf>,
}

/// Top-level daemon configuration loaded from config.toml.
#[derive(Debug, Clone, Deserialize)]
pub struct DaemonConfig {
    /// Unix domain socket path. Default: `~/.local/state/text-runtime/runtime.sock`
    #[serde(default = "default_socket_path")]
    pub socket_path: PathBuf,
    /// PID file path. Default: `~/.local/state/text-runtime/runtime.pid`
    #[serde(default = "default_pid_path")]
    pub pid_path: PathBuf,
    /// Grace period in seconds to wait for in-flight connections on shutdown.
    #[serde(default = "default_shutdown_grace")]
    pub shutdown_grace_seconds: u64,
    /// Configured workspaces (loaded from config; more can be added at runtime).
    #[serde(default)]
    pub workspaces: Vec<WorkspaceConfig>,
}

// ── Default value functions (used by serde) ──────────────────────────────────

fn default_socket_path() -> PathBuf {
    state_dir().join("runtime.sock")
}

fn default_pid_path() -> PathBuf {
    state_dir().join("runtime.pid")
}

fn default_shutdown_grace() -> u64 {
    10
}

/// XDG-compliant state directory: `$XDG_STATE_HOME/text-runtime` or
/// `~/.local/state/text-runtime` as fallback.
fn state_dir() -> PathBuf {
    std::env::var("XDG_STATE_HOME")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".local")
                .join("state")
        })
        .join("text-runtime")
}

/// XDG-compliant config directory: `$XDG_CONFIG_HOME/text-runtime` or
/// `~/.config/text-runtime` as fallback.
fn config_dir() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".config")
        })
        .join("text-runtime")
}

// ── ConfigHandle ─────────────────────────────────────────────────────────────

/// Thread-safe config wrapper using ArcSwap for lock-free hot-reload.
///
/// Existing connections call `load()` once and hold their `Arc<DaemonConfig>`
/// for the duration of the request. When SIGHUP fires, `store()` atomically
/// swaps in the new config; new connections see it immediately.
pub struct ConfigHandle {
    inner: ArcSwap<DaemonConfig>,
}

impl ConfigHandle {
    pub fn new(config: DaemonConfig) -> Self {
        Self {
            inner: ArcSwap::from_pointee(config),
        }
    }

    /// Load the current config snapshot. Cheap — just an atomic load + Arc clone.
    pub fn load(&self) -> Arc<DaemonConfig> {
        self.inner.load_full()
    }

    /// Atomically swap in a new config. Existing holders keep the old Arc.
    pub fn store(&self, config: DaemonConfig) {
        self.inner.store(Arc::new(config));
    }
}

// ── load_config ───────────────────────────────────────────────────────────────

/// Load daemon configuration from a TOML file.
///
/// If `path` is `None`, uses `~/.config/text-runtime/config.toml`.
/// If the file does not exist, returns a `DaemonConfig` with all defaults
/// (zero workspaces — workspaces can be added at runtime via `workspace_add`).
///
/// Expands `~` in all path fields after parsing.
pub fn load_config(path: Option<&Path>) -> Result<DaemonConfig, TextRuntimeError> {
    let config_path = path
        .map(PathBuf::from)
        .unwrap_or_else(|| config_dir().join("config.toml"));

    if !config_path.exists() {
        // No config file — start with all defaults (zero workspaces).
        return Ok(DaemonConfig {
            socket_path: default_socket_path(),
            pid_path: default_pid_path(),
            shutdown_grace_seconds: default_shutdown_grace(),
            workspaces: Vec::new(),
        });
    }

    let raw =
        std::fs::read_to_string(&config_path).map_err(|e| TextRuntimeError::io(&config_path, e))?;

    let mut config: DaemonConfig = toml::from_str(&raw).map_err(|e| {
        TextRuntimeError::ConfigError(format!("failed to parse {}: {}", config_path.display(), e))
    })?;

    // Expand ~ in all path fields
    config.socket_path = expand_tilde(config.socket_path);
    config.pid_path = expand_tilde(config.pid_path);
    for ws in &mut config.workspaces {
        ws.root = expand_tilde(ws.root.clone());
        ws.data_dir = expand_tilde(ws.data_dir.clone());
        ws.watch_dirs = ws.watch_dirs.drain(..).map(expand_tilde).collect();
    }

    Ok(config)
}

/// Expand a leading `~` to the user's home directory.
pub fn expand_tilde(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    if s.starts_with("~/") || s == "~" {
        if let Some(home) = dirs::home_dir() {
            return home.join(&s[2..]);
        }
    }
    path
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_no_file() {
        // load_config with a path that doesn't exist should return defaults
        let tmp = tempfile::tempdir().unwrap();
        let nonexistent = tmp.path().join("config.toml");
        let config = load_config(Some(&nonexistent)).unwrap();
        assert!(config.workspaces.is_empty());
        assert_eq!(config.shutdown_grace_seconds, 10);
    }

    #[test]
    fn test_parse_minimal_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"
[[workspaces]]
name = "notes"
root = "/home/user/notes"
data_dir = "/home/user/.local/share/text-runtime/notes"
"#,
        )
        .unwrap();

        let config = load_config(Some(&config_path)).unwrap();
        assert_eq!(config.workspaces.len(), 1);
        assert_eq!(config.workspaces[0].name, "notes");
    }

    #[test]
    fn test_parse_full_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.toml");
        let sock = tmp.path().join("runtime.sock");
        let pid = tmp.path().join("runtime.pid");
        std::fs::write(
            &config_path,
            format!(
                r#"
socket_path = "{}"
pid_path = "{}"
shutdown_grace_seconds = 30

[[workspaces]]
name = "work"
root = "/home/user/work"
data_dir = "/home/user/.data/work"
watch_dirs = ["/home/user/work/docs"]
"#,
                sock.display(),
                pid.display()
            ),
        )
        .unwrap();

        let config = load_config(Some(&config_path)).unwrap();
        assert_eq!(config.shutdown_grace_seconds, 30);
        assert_eq!(config.workspaces[0].watch_dirs.len(), 1);
    }

    #[test]
    fn test_config_handle_swap() {
        let cfg1 = DaemonConfig {
            socket_path: PathBuf::from("/tmp/a.sock"),
            pid_path: PathBuf::from("/tmp/a.pid"),
            shutdown_grace_seconds: 5,
            workspaces: vec![],
        };
        let cfg2 = DaemonConfig {
            socket_path: PathBuf::from("/tmp/b.sock"),
            pid_path: PathBuf::from("/tmp/a.pid"),
            shutdown_grace_seconds: 10,
            workspaces: vec![],
        };

        let handle = ConfigHandle::new(cfg1);
        assert_eq!(handle.load().shutdown_grace_seconds, 5);

        handle.store(cfg2);
        assert_eq!(handle.load().shutdown_grace_seconds, 10);
        assert_eq!(handle.load().socket_path, PathBuf::from("/tmp/b.sock"));
    }

    #[test]
    fn test_expand_tilde() {
        let home = dirs::home_dir().unwrap();
        let expanded = expand_tilde(PathBuf::from("~/documents/notes"));
        assert!(expanded.starts_with(&home));
        assert!(expanded.ends_with("documents/notes"));
    }
}
