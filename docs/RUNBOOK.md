# Text Runtime — Production Runbook

Operational guide for running the `text-runtime` engine and its daemon in
production (i.e. persistently, on a real machine, serving agents).

---

## 1. Components

| Component | What it is | Runs as |
|---|---|---|
| `text-runtime` (Rust) | Engine: ingest → structure → annotate → search. CLI + daemon | Binary / background service |
| `text-runtime daemon` | Unix-socket server + file watcher + SIGHUP config reload | launchd (macOS) / systemd (Linux) |
| `text-runtime-mcp` (Python) | MCP server for goose/Berd agents; shells out to the engine binary | stdio child of the agent |

The daemon is optional: the MCP server manages its own pandoc-server and can
point at any runtime dir. Run the daemon when you want long-lived workspaces
with automatic file watching.

## 2. Install & upgrade

```bash
# Build + install to ~/.local/bin (add to PATH)
./scripts/install.sh

# Quality gates + release build + version tag
./scripts/release.sh            # gates only
./scripts/release.sh --tag 0.2.0   # gates + git tag v0.2.0
```

Upgrade procedure:

1. `git pull` (or fetch the new release artifact)
2. `./scripts/release.sh` — confirms fmt/clippy/tests pass on the new code
3. `./scripts/install.sh` — installs the new binary
4. Restart the service (below)
5. Smoke-test: `text-runtime --version`, then `text-runtime read <doc-uuid>`

The MCP server (`text-runtime-mcp`) pins the engine via `TEXT_RUNTIME_BIN`;
make sure it points at the installed binary, not a stale `target/release` path.

## 3. Config

Daemon config file: `~/.config/text-runtime/config.toml` (XDG-aware;
`$XDG_CONFIG_HOME/text-runtime/config.toml`). Example:

```toml
# socket_path = "~/.local/state/text-runtime/runtime.sock"   # defaults
# pid_path    = "~/.local/state/text-runtime/runtime.pid"
shutdown_grace_seconds = 5

[[workspaces]]
name = "notes"
root = "/Users/me/notes"
data_dir = "/Users/me/.local/share/text-runtime/notes"
watch_dirs = ["/Users/me/notes"]
```

- `watch_dirs` empty = no auto re-ingestion; workspaces still accessible via IPC.
- Config is hot-reloaded on `SIGHUP`: workspaces added/removed in the file are
  applied. Runtime-added workspaces are **removed** on reload if absent from
  config — keep the file authoritative.
- Runtime data layout (`data_dir/.textruntime/`): `db.sqlite` (structure,
  annotations, FTS5) + `content/{00-ff}/` (Pandoc AST JSON, one file per block
  node) + `tmp/` (atomic-write staging). The `config.json` inside sets the
  pandoc-server port for that runtime.

## 4. Service lifecycle

### macOS (launchd)

```bash
cp deploy/text-runtime.plist ~/Library/LaunchAgents/
launchctl load ~/Library/LaunchAgents/text-runtime.plist     # start now + at login
launchctl unload ~/Library/LaunchAgents/text-runtime.plist   # stop
```

`KeepAlive=true` restarts the daemon on crash. Logs: `/tmp/text-runtime-daemon.log`
or `log stream --predicate 'process == "text-runtime"'`.

### Linux (systemd)

```bash
sudo cp deploy/text-runtime.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now text-runtime
journalctl -u text-runtime -f
```

### Single-instance enforcement

The daemon takes a single-instance lock (flock on the PID file on macOS;
abstract Unix socket on Linux). A second instance exits with
`DaemonAlreadyRunning` — do not fight it; use `launchctl`/`systemctl` status.

## 5. Health checks

```bash
# Daemon socket responds? (IPC protocol is newline-delimited JSON)
printf '{"id":"ping","cmd":"status"}\n' | \
  nc -U ~/.local/state/text-runtime/runtime.sock

# Pandoc server up? (each runtime spawns/manages its own)
curl -sf http://127.0.0.1:8472/version
```

Expected daemon status fields: version, start time, workspace list.

## 6. Backup & restore

The store is two parts that must be backed up **together**:

```bash
# While the daemon is stopped (or use SQLite backup to avoid WAL races):
tar czf text-runtime-backup.tgz \
  ~/.local/share/text-runtime/          # all workspace data dirs
# plus any config: ~/.config/text-runtime/config.toml
```

Restore: unpack to the same paths, start the daemon. UUIDs are stable, so
annotations and transclusions remain valid as long as both `db.sqlite` and
`content/` are restored from the same snapshot.

## 7. Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `DaemonAlreadyRunning` | Another instance holds the lock | Stop it via launchctl/systemctl; stale PID file is cleaned on drop |
| Socket connect refused | Daemon not running | Check service status; `KeepAlive`/`Restart=on-failure` should have recovered it |
| Ingest hangs | pandoc-server wedged | The runtime restarts it with backoff (max 10). Check `ps aux \| grep pandoc`; kill strays on the runtime's port |
| `EACCES` on runtime dirs | Directory created with a bad mode (older bug) | `chmod -R u+rwX` the `.textruntime` dir; the umask bug is fixed in current builds |
| Search returns nothing | FTS index stale | Re-ingest the document; FTS5 is content-sync via triggers |
| MCP tool errors `PandocServerNotRunning` | `TEXT_RUNTIME_BIN` points at a stale binary | Point it at `~/.local/bin/text-runtime` (see §2) |

## 8. Observability

- Structured logging via `tracing`; the daemon writes to stderr (captured by
  launchd/systemd).
- No metrics endpoint yet. For fleet usage, watch: socket accept failures,
  pandoc restart count, ingest durations (activity_logger table in SQLite).
- The activity log (`activities` table) records every ingest/re-ingest —
  query it for an audit trail: `text-runtime` does not expose a CLI for it yet;
  use `sqlite3 <data_dir>/.textruntime/db.sqlite "select * from activities"`.

## 9. Known gaps (roadmap)

- No metrics/health HTTP endpoint on the daemon (IPC socket only).
- No cross-document relationship queries exposed over IPC.
- Daemon watcher is best-effort; very rapid file bursts may coalesce.
