# Text Runtime — System Architecture

**Date:** 2026-07-11
**Status:** Implemented (Phase 1 Engine + Phase 2 Daemon)

---

## What This Is

The Text Runtime is a persistent layer beneath text. It treats text not as an opaque character stream but as a collection of addressable, stable objects — every sentence, paragraph, and heading gets a UUID that survives edits, enables stable cross-referencing, and gives AI agents a deterministic way to point at and annotate content.

The human-facing text stays clean. The runtime works behind it.

---

## Core Principles

1. **Markdown remains the interface.** Humans write Markdown. LLMs read Markdown. LLMs generate Markdown. No new authoring language is introduced.

2. **The parser is not an authority.** Parsing is best-effort. The runtime never assumes parsing is perfect. Instead it produces a hypothesis, records uncertainty, and preserves as much structure as possible. The parser never fails — it degrades gracefully.

3. **Identity is independent from position.** Nothing is identified by order (paragraph 17, sentence 9, line 241). Every node receives a UUID v7. Ordering is stored separately as a list of child UUIDs. Identity never depends on ordering.

4. **Structure is deterministic.** The runtime builds only structural hierarchy (document → section → paragraph → sentence). No AI, no ontology, no semantic reasoning. Headings create sections, blank lines create paragraphs, sentence tokenizer creates sentences.

5. **The source file is a provenance record, not the source of truth.** After ingest, the runtime's copy is canonical. Deleting or renaming the source file has no effect on the runtime's integrity.

---

## Architecture: Three Layers

```
Applications
────────────────────────────────────────────
CLI, Viewer, MCP Server, VS Code Extension,
Pi SDK Tools, future integrations
            │ (IPC over Unix socket)
Runtime (Daemon)
────────────────────────────────────────────
Workspace management, filesystem watching,
identity resolution, store management,
configuration, IPC server
            │
Semantic Engine (Rust Library)
────────────────────────────────────────────
Parsing, normalization, object model,
UUID generation, persistence, query,
reconstruction, search
```

### Layer 1: Semantic Engine (Phase 1 — Complete)

A Rust library that owns all document processing. Platform-independent — knows nothing about filesystems, agents, or operating systems.

**Responsibilities:**
- Ingest documents (Markdown, TXT, HTML, LaTeX, etc.)
- Parse into structural tree via Pandoc AST
- Segment into document → section → paragraph → sentence
- Assign UUID v7 to every node
- Persist to SQLite (structure, identity) + Pandoc AST files (content)
- Reconstruct documents and project to any output format
- Full-text search via FTS5
- Annotation storage and re-anchoring (W3C Web Annotation model)

### Layer 2: Runtime Daemon (Phase 2 — Complete)

A persistent daemon that wraps the engine and interacts with the operating system.

**Responsibilities:**
- Workspace management (add/remove/list workspaces)
- Filesystem watching with debounced, SHA-256 change detection
- IPC server over Unix domain socket (NDJSON protocol)
- 11 daemon commands: `workspace_list`, `workspace_add`, `workspace_remove`, `ingest`, `ingest_text`, `read`, `annotate`, `search`, `toc`, `status`, `shutdown`
- Single-instance enforcement via `flock` (macOS) / abstract socket (Linux)
- Graceful lifecycle with SIGINT/SIGTERM handling, connection draining, WAL sync
- SIGHUP hot-reload for configuration changes

### Layer 3: Applications (Future)

CLI, viewer, MCP server, VS Code extension, Pi SDK tools — applications that connect to the daemon over the Unix socket and use its 11 commands.

---

## Data Model

### Two-Store Architecture

```
Source (any format)
        ↓ ingest

.textruntime/
  db.sqlite                     ← structure, identity, annotations, transclusions, activities, FTS index
                                    NO TEXT CONTENT HERE

  content/{fanout}/{uuid}.json  ← Pandoc AST fragment per block-level node
                                    format-agnostic, runtime-owned
                                    projects to any format via Pandoc

        ↓ project

Any format (Markdown, HTML, PDF, LaTeX, Djot, ...)
```

**Why:** SQLite is for structured, relational, queryable data (identities, types, parent-child links, annotation state). Pandoc AST files on the filesystem hold text content. Text never goes in SQL.

### Node types and content storage

| Node type | Content file? | Text source |
|---|---|---|
| `sentence` | ❌ | Derived from parent paragraph by `char_start`/`char_end` |
| `paragraph` | ✅ | Pandoc `Para` block |
| `heading` | ✅ | Pandoc `Header` block with level |
| `code_block` | ✅ | Pandoc `CodeBlock` with language |
| `list_item` | ✅ | Pandoc list item block |
| `section` | ❌ | Assembled from children on demand |
| `document` | ❌ | Assembled from children on demand |

### Agent Interface

The runtime uses a two-step protocol for agent interaction:

1. **Read**: Project a document with §N sentence markers for scannability. Returns `{ text, marker_map }` where `marker_map` maps §N numbers → sentence UUIDs.

2. **Annotate**: The client resolves §N → UUID using the marker_map, then sends the UUID directly to `annotate`. The daemon is stateless — it never stores per-connection §N state.

This solves the "cursor problem": agents cannot point by position (they hallucinate offsets) and cannot see UUIDs inline (too much token noise). §N markers are lightweight (~3 chars per sentence), and UUIDs are resolved client-side from the marker_map.

### W3C Web Annotations

All annotations follow the W3C Web Annotation Data Model with dual selectors:
- **TextPositionSelector** (character offsets — fast when unchanged)
- **TextQuoteSelector** (exact text + prefix/suffix — recovers after edits)

Selectors are reconciled at write time so they describe the same span. Annotation state machine: `active → active_partial → orphan → deleted`.

---

## Pipeline

```
Source (text string or file)
        ↓
Format Normalizer
  detect format, normalize unicode, normalize line endings
        ↓
Pandoc Parser
  parse to full Pandoc AST
        ↓
Structural Segmenter
  walk Pandoc AST → document → sections → paragraphs → sentences
  assign structural position
        ↓
UUID Assigner
  compute structural_hash per node (SHA-256)
  match against existing hashes (re-ingest diff)
  keep existing UUID or generate new UUID v7
        ↓
Content Writer
  for each block node: write tmp/{uuid}.json, fsync, rename
  sentence nodes: char_start/char_end in SQLite only
        ↓
SQLite Writer
  INSERT or UPDATE documents, nodes
  UPDATE deleted nodes, orphaned annotations/transclusions
        ↓
FTS Indexer
  extract plain text from Pandoc AST, update nodes_fts
        ↓
Activity Logger
  INSERT activity record (append-only, never updated)
```

---

## Current State

### Phase 1 (Engine) — Complete
- Document parsing and structural tree construction
- UUID v7 assignment with collision detection and deduplication
- SQLite persistence with FTS5 full-text search
- Content file store with Pandoc AST, directory fanout, atomic writes
- Document projection to any output format via Pandoc
- W3C Web Annotation support with dual-selector anchoring
- Re-ingestion protocol: hash-based change detection, UUID preservation for unchanged/fuzzy-matched nodes
- §N sentence marker injection with marker_map for agent interface
- 141+ unit tests passing

### Phase 2 (Daemon) — Complete
- 10-module daemon architecture fully implemented
- 11-command IPC protocol over Unix domain socket (NDJSON)
- Dynamic workspace management with DashMap registry
- Debounced filesystem watchers with SHA-256 change pre-filtering
- Single-instance enforcement (POSIX locks)
- Graceful lifecycle: SIGINT/SIGTERM, connection draining, WAL sync
- SIGHUP hot-reload for configuration
- Verified with 4 heavyweight E2E test scenarios:
  - Rapid Research: concurrent file writes, ingest, search, annotation
  - Chaos & Concurrency: 5 concurrent clients, connection drops
  - Dynamic Hot-Reload: mock config, runtime workspace addition
  - Recovery & Unlink: singleton enforcement, socket/PID cleanup

### Phase 3 (Applications) — Planned
- CLI client
- MCP Server
- VS Code Extension
- Pi SDK tooling
- Viewer
- Annotation management tools

---

## Reading Guide

### For a newcomer
1. This document (5 min) — What it is, core principles, architecture
2. `storage-architecture.md` §1-2 (5 min) — Core principle, two-store architecture
3. `schema.md` (5 min) — Database schema and design decisions

### For an implementer
1. `storage-architecture.md` — Full 15-section specification
2. `daemon-architecture.md` — Daemon module design
3. `daemon-protocol.md` — Wire protocol (API reference)
4. `annotations.md` — W3C annotation implementation
5. `inline-offset-mapping.md` — §N marker injection algorithm
6. `text-runtime/src/` — The Rust implementation

### For an architecture reviewer
1. This document (10 min) — System overview
2. `storage-architecture.md` §3-7, 9-11 — Most innovative sections
3. `schema.md` — Schema design decisions
4. `annotations.md` — W3C annotation model adaptation
5. `daemon-architecture.md` §10 — Post-implementation notes

### For an AI agent
1. `daemon-protocol.md` — The wire protocol (what you call)
2. `storage-architecture.md` §11 — Agent interface (marker_map, UUID annotation)
3. `daemon-architecture.md` §10 — Implementation notes (edge cases)

---

## Related Documents

| Document | Purpose |
|----------|---------|
| `storage-architecture.md` | Canonical storage design (two-store, Pandoc AST, W3C annotations, agent interface, re-ingestion protocol) |
| `daemon-architecture.md` | Daemon module design, lifecycle, locking, implementation notes |
| `daemon-protocol.md` | Wire protocol: 11 commands, NDJSON framing, client examples |
| `annotations.md` | W3C Web Annotation: Rust types, selector resolution, status state machine |
| `schema.md` | SQLite schema with design decisions (FTS5 compatibility, dual text storage, denormalized hierarchy) |
| `inline-offset-mapping.md` | Inline-to-offset mapping algorithm, §N marker injection, verification tests |

