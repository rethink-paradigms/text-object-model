# Engine Status — Text Runtime

**Date**: 2026-07-10
**Status**: Core engine built and tested

---

## What Is Built

A local-first Rust binary: `text-runtime`

### Rust project
```
workspace/text-object-model/text-runtime/
  src/                    28 source files, ~9,300 lines of Rust
  Cargo.toml              all dependencies locked
  tests/                  integration test suite
  test-fixtures/          real .textruntime/ with content files + db.sqlite
```

### Module breakdown
```
src/
  cfg.rs            — RuntimeConfig, .textruntime/ directory layout
  types.rs          — StructuralNode, DocumentRecord, all core types
  uuid7.rs          — UUID v7 generator
  error.rs          — TextRuntimeError enum
  pandoc_mgr.rs     — PandocManager: spawn, health-check, /batch API
  runtime.rs        — TextRuntime: top-level orchestrator
  reingest.rs       — re-ingestion diff: exact → fuzzy → new → deleted
  projection.rs     — project to Markdown / plaintext / HTML with §N markers
  transclusion.rs   — TransclusionRecord, 8 typed predicates
  agent.rs          — agent command/response protocol (§N interface)
  daemon.rs         — file-watcher daemon mode
  lib.rs            — public API surface
  main.rs           — CLI entry point
  pipeline/
    format.rs       — format detection + normalization
    ingest.rs       — full ingestion pipeline
    inline_mapping.rs — sentence boundary → Pandoc inline offset mapping
    content_writer.rs — atomic content file writes (tmp → fsync → rename)
    sqlite_writer.rs  — all SQLite writes in single transaction
    fts_indexer.rs    — FTS5 content-sync indexer
  store/
    (SQLite schema, migrations, queries)
  annotation/
    types.rs        — W3C annotation serde types
    reconcile.rs    — dual selector reconciliation
    anchoring.rs    — 4-strategy re-anchoring cascade
```

### Dependencies
- `pandoc_ast 0.8` — Pandoc AST types
- `icu_segmenter 2.2` — Unicode Consortium sentence segmentation
- `rusqlite 0.31` — SQLite with bundled FTS5
- `reqwest 0.12` — pandoc-server HTTP client
- `uuid 1` — UUID v7
- `sha2 0.10` — structural hashing
- `clap 4` — CLI
- `serde + serde_json 1` — serialization
- `tokio 1` — async runtime
- `notify 7` — file watching (daemon mode)
- `thiserror 2` — error types
- `chrono 0.4` — timestamps

---

## Test Status

**141/142 unit tests pass.**

The 1 "failing" test: `test_health_check_returns_false_when_no_server` — not a code bug. pandoc-server is running on port 8472 (launched by prior sessions), so the health check correctly returns `Ok(true)`. The test assumes no server is running. Harmless — disappears when pandoc-server is not running.

---

## Format Testing (Antigravity)

Tested by Antigravity (Google's coding agent) with 10+ formats:
- Complex Markdown with nested lists, tables, code blocks, footnotes
- Mixed formatting (bold inside italic inside links)
- Multi-byte Unicode, CJK characters, math expressions
- Long paragraphs, single-sentence paragraphs
- Empty sections, documents with only headings
- Documents with only code blocks (no prose)

**All formats working. Nothing breaking.**

---

## What pandoc-server Does

pandoc-server runs as a managed child process. The runtime:
1. Spawns it on startup if not already running
2. Health-checks via HTTP GET `/`
3. Converts documents via POST `/batch` (JSON array of conversion requests)
4. Shuts it down on runtime exit

Pandoc supports 40+ input formats — the runtime inherits this for free. Any text Pandoc can parse, the runtime can ingest.

---

## Two-Store Architecture (as built)

```
.textruntime/
  db.sqlite                       — identity, structure, annotations,
                                    transclusions, activities, FTS5
                                    NO text content here
  content/
    {00-ff}/                      — 256-bucket directory fanout
      {uuid}.json                 — Pandoc AST fragment per block node
                                    text lives here, format-agnostic
  tmp/                            — atomic write staging
  config.json
```

Block nodes (paragraph, heading, code_block, list_item, table) → content files.
Sentence nodes → SQLite rows only (char_start/char_end into parent paragraph).

---

## Re-Ingestion (as built)

When a document is edited and re-ingested, the runtime diffs the new tree against the stored tree:

1. **Exact match** — structural hash matches, position within tolerance → keep UUID
2. **Fuzzy match** — text similarity above threshold, position within tolerance → keep UUID, update text
3. **New node** — no match → mint new UUID v7
4. **Deleted node** — existing UUID not matched → mark deleted, keep record (annotations move to orphan state)

---

## What Is NOT Built

| Component | Notes |
|---|---|
| Discovery interface | Corpus overview, annotation landscape, topic clusters |
| Structured read brief | Document + epistemic state projected together |
| Output declaration | Agent declares provenance of what it wrote |
| Pi SDK tool wrapper | The extension that lets fleet agents call the runtime |
| Cross-document search | FTS5 exists; cross-doc relationship queries not exposed |
| Daemon mode (production) | Code exists, needs integration testing |

---

## Next Build Targets

1. Pi SDK tool wrapper — `tools/internal/text-runtime/` extension
2. Discovery interface — `corpus_overview()`, `annotation_landscape()` API methods
3. Structured read brief — augment `project()` to include epistemic state
4. Output declaration — `declare_output()` API method with TROVE relationship types
