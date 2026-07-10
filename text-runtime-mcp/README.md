# text-runtime-mcp

MCP server exposing the [text-runtime](../text-runtime) engine to goose/Berd agents.

The engine stays the single source of truth; this layer is a thin agent-facing
surface that shells out to the `text-runtime` release binary.

## Tools

| Tool | What it does |
|---|---|
| `ingest_document` | Ingest a file (markdown, txt, html, org, rst, ...) → doc UUID |
| `list_documents` | Corpus overview: uuid, title, format, ingested_at, version |
| `read_document` | Project a doc back to markdown/html/plain text |
| `document_sentences` | Sentence anchors: `index | sentence_uuid | text` (index = §N position) |
| `annotate_sentence` | Attach a W3C annotation to a sentence UUID |
| `search_corpus` | FTS5 full-text search with ranked hits |

## How it works

- Each tool shells out to `text-runtime/target/release/text-runtime` (rebuild after Rust changes: `cargo build --release` in `../text-runtime`).
- The server owns a **persistent pandoc-server on port 8499** (own child process, restarted if wedged) and points each runtime dir's `config.json` at it — avoiding the shared-port-8472 hangs from orphaned pandoc servers.
- Default corpus: `~/.textruntime` (overridable per call via `runtime_dir`, or globally via `TEXT_RUNTIME_DIR`).
- Sentence UUIDs are stable across re-ingests; §N indices are positional and may shift.

## Environment

| Var | Default |
|---|---|
| `TEXT_RUNTIME_BIN` | `<repo>/text-runtime/target/release/text-runtime` |
| `TEXT_RUNTIME_DIR` | `~/.textruntime` |
| `TEXT_RUNTIME_PANDOC_PORT` | `8499` |

## Dev

```bash
uv sync
uv run python smoke_test.py   # MCP handshake + tools/list + one call
uv run python e2e_test.py     # ingest → list → read → sentences → annotate → search
```

## Registration

Registered in `~/.config/goose/config.yaml` as a `stdio` extension:

```yaml
text-runtime:
  type: stdio
  name: text-runtime
  enabled: true
  cmd: /abs/path/to/text-runtime-mcp/.venv/bin/python
  args: ["-m", "text_runtime_mcp"]
```

Appears in the extension manager after restarting Berd/goose.
