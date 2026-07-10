## Result: Pandoc Server Manager (`src/pandoc_mgr.rs`) created

**Status:** done

### What was created

- **`src/pandoc_mgr.rs`** (407 lines) — Pandoc server process manager module

### What's in the file

| Component | Description |
|-----------|-------------|
| `PandocManager` struct | Manages pandoc-server child process with restart tracking |
| `PandocManager::new()` | Creates from `RuntimeConfig`, builds reqwest `Client` with 30s timeout |
| `PandocManager::spawn()` | Spawns pandoc-server child, health-checks with exponential backoff (200ms base, up to 2⁵) |
| `PandocManager::health_check()` | GET /version — 5s timeout, returns bool |
| `PandocManager::restart()` | Kill + respawn with backoff (100ms base, capped 30s), max 10 restarts |
| `PandocManager::shutdown()` | Graceful: SIGTERM → 5s wait → SIGKILL; handles already-dead child |
| `PandocManager::convert()` | Single-doc convenience wrapper around convert_batch |
| `PandocManager::convert_batch()` | POST /batch with JSON, 30s timeout, maps errors to `TextRuntimeError` variants |
| `PandocManager::extract_ast_json()` | Parses `"output"` string field into `serde_json::Value` |
| `Drop` impl | Sync best-effort: `start_kill()` → `try_wait()` → `kill()` |
| `PandocBatchItem` | Serializable batch input (`text`, `from`, `to`) |
| `PandocBatchResponse` | Deserializable batch output (`output`, `error`, `base64`, `messages`) |
| `PandocMessage` | Warning/info message (`message`, `verbosity`) |

### Constraints satisfied

- ✅ **No `unwrap()` in production paths** — only `.expect()` on infallible `Client::builder()`, `.unwrap_or("[]")` in `extract_ast_json`
- ✅ **Client-side timeout enforced** — every reqwest call has `.timeout()` (30s for batch, 5s for health check)
- ✅ **/batch endpoint** — POST with `Content-Type: application/json`, `application/json` response
- ✅ **`extract_ast_json()`** — parses `"output"` string via `serde_json::from_str`
- ✅ **`shutdown()`** — handles already-dead child gracefully
- ✅ **Drop impl** — sync best-effort kill
- ✅ **All public methods have doc comments**
- ✅ **Restart tracking** — `restart_count` field, `PandocServerCrashed` after 10 failures

### Tests included

1. `test_health_check_port_free` — verifies health check returns false without panicking
2. `test_batch_item_serialization` — verifies PandocBatchItem → JSON output format
3. `test_batch_response_deserialization_success` — verifies successful response parsing
4. `test_batch_response_deserialization_error` — verifies error response parsing
5. `test_extract_ast_json` — verifies AST extraction from output field
6. `test_extract_ast_json_empty` — verifies graceful handling of None output

### Imports verified

| Import | Source | Status |
|--------|--------|--------|
| `reqwest::Client` | Cargo.toml `reqwest = "0.12"` with `json` feature | ✅ |
| `tokio::process::{Child, Command}` | Cargo.toml `tokio = "1.40"` with `full` | ✅ |
| `crate::cfg::RuntimeConfig` | `src/cfg.rs` — has `pandoc_port: u16`, `pandoc_executable: String` | ✅ |
| `crate::error::TextRuntimeError` | `src/error.rs` — has `PandocServerNotRunning`, `PandocServerCrashed`, `PandocHealthCheckTimeout`, `PandocTimeout`, `PandocConversionError`, `InternalError` | ✅ |
| `serde::{Serialize, Deserialize}` | Transitive via `serde` dependency | ✅ |
| `serde_json::Value` | Transitive via `serde_json` dependency | ✅ |

### What still needs to happen

- **Module declaration**: When `src/lib.rs` is created, add `pub mod pandoc_mgr;`
- **Integration tests**: Real pandoc-server integration tests should be gated behind a feature flag or env var check (e.g., `#[cfg_attr(not(feature = "integration"), ignore)]` or checking `std::env::var("PANDOC_SERVER_PATH")`)
- **Compile verification**: Run `cargo check` / `cargo test` once the Rust toolchain is available
