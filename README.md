# text-object-model

[![CI](https://github.com/rethink-paradigms/text-object-model/actions/workflows/ci.yml/badge.svg)](https://github.com/rethink-paradigms/text-object-model/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)
[![Version](https://img.shields.io/badge/version-0.1.0-blue.svg)](https://github.com/rethink-paradigms/text-object-model/releases)

Local-first text runtime with an agent-facing MCP layer: ingest, structure,
annotate, and project text documents with UUID-stable identity, W3C Web
Annotations, and content-addressed persistence.

## Components

### `text-runtime/` — Rust engine

Core engine (library + CLI + daemon). Parses Markdown, LaTeX, HTML, DOCX and
more via pandoc, segments text into sentences, assigns UUID v7 identities,
stores structure in SQLite plus content-addressed files, and projects
documents back in any format with §N markers. Also supports W3C annotations,
FTS5 search, and typed transclusions.

Quick commands (from `text-runtime/`):

```bash
cargo run -- ingest path/to/document.md
cargo run -- read <doc-uuid> --format markdown --markers
cargo run -- search "query"
cargo run -- daemon --config ~/.config/text-runtime/config.toml
```

See `text-runtime/README.md` for the full reference.

### `text-runtime-mcp/` — MCP server

Python MCP server (uv-managed, Python ≥ 3.13) exposing the engine to
goose/Berd agents as a `stdio` extension. Each tool shells out to the Rust
release binary; the server owns a persistent `pandoc-server` on port 8499.

Register it in `~/.config/goose/config.yaml`:

```yaml
text-runtime:
  type: stdio
  name: text-runtime
  enabled: true
  cmd: /abs/path/to/text-runtime-mcp/.venv/bin/python
  args: ["-m", "text_runtime_mcp"]
```

Config via `TEXT_RUNTIME_BIN`, `TEXT_RUNTIME_DIR`, `TEXT_RUNTIME_PANDOC_PORT`.

### `site/` — research site

Static HTML research site documenting the project's architecture and design.

## Development

Requirements: Rust 1.82+, pandoc, uv (Python 3.13).

- **Rust**: `cd text-runtime && cargo test` (integration tests require pandoc;
  the crate falls back from `pandoc-server` to `pandoc server`).
- **MCP**: build the release binary first, then
  `uv sync --project text-runtime-mcp` and run
  `TEXT_RUNTIME_BIN=text-runtime/target/release/text-runtime uv run --project text-runtime-mcp python text-runtime-mcp/smoke_test.py`
  (and `e2e_test.py`).
- **CI**: `.github/workflows/ci.yml` — the `rust` job runs fmt, clippy, tests
  and the release build; the `mcp` job reuses the built binary for smoke/e2e
  tests.

## License

MIT OR Apache-2.0
