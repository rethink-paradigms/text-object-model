# Text Runtime — SQLite Schema

**Status:** Production specification
**Sources:** SiSU (20+ years production), rusqlite best practices, FTS5 content-sync pattern, consultation findings

## Design Decisions

### 1. INTEGER PRIMARY KEY for FTS5 compatibility

FTS5's `content=` sync mode requires an integer rowid that matches the content table's primary key. UUIDs are text, not integers. Therefore every table gets **both** an auto-incrementing integer PK (internal, for FTS5) and a UUID column (external, for stable addressing).

```sql
-- Internal: INTEGER PRIMARY KEY → FTS5 content_rowid
-- External: uuid TEXT UNIQUE → stable identity, never changes
id    INTEGER PRIMARY KEY AUTOINCREMENT
uuid  TEXT NOT NULL UNIQUE
```

The integer PK is never exposed outside the runtime. The UUID is the only address that agents and humans see.

### 2. Dual text storage (from SiSU)

SiSU stores both `clean` (plain text) and `body` (formatted) per object. This pattern is adopted:
- `plain_text` — extracted text for FTS indexing and sentence segmentation
- The content FILE stores the full Pandoc AST JSON (formatted representation)

The `plain_text` column is the key innovation: it's extracted once at ingest time, stored in the nodes table, and used for both FTS indexing and sentence segmentation. This avoids re-extracting from Pandoc AST on every search query.

### 3. Denormalized heading hierarchy (from SiSU)

SiSU stores `lev0` through `lev7` as columns on each object for fast section-level queries without recursive CTEs. Adopted here as `heading_level` and `section_path`.

### 4. Hashes for change detection (from SiSU and storage architecture)

SiSU's `digest_clean` / `digest_all` pattern is identical to our `structural_hash`. Confirmed as production-grade — 20+ years of use.

## Full Schema

```sql
-- ── Core: Documents ──────────────────────────────────────────────

CREATE TABLE documents (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    uuid            TEXT NOT NULL UNIQUE,           -- UUID v7, stable external identity
    title           TEXT NOT NULL DEFAULT '',
    import_format   TEXT NOT NULL,                  -- "markdown" | "docx" | "latex" | "html" | "epub" | ...
    import_path     TEXT,                           -- provenance: where the source came from
    import_hash     TEXT,                           -- SHA-256 of source at last ingest (change detection)
    root_node_uuid  TEXT,                           -- UUID of the document root node
    version         INTEGER NOT NULL DEFAULT 1,
    ingested_at     TEXT NOT NULL,                  -- ISO 8601
    language        TEXT DEFAULT 'en'               -- for locale-aware segmentation
);

CREATE UNIQUE INDEX idx_documents_uuid ON documents(uuid);

-- ── Core: Nodes (the structural tree) ──────────────────────────

CREATE TABLE nodes (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    uuid            TEXT NOT NULL UNIQUE,           -- UUID v7, stable forever
    doc_id          TEXT NOT NULL REFERENCES documents(uuid),
    node_type       TEXT NOT NULL,                  -- "document" | "section" | "paragraph" | "heading"
                                                    -- | "sentence" | "code_block" | "list_item"
                                                    -- | "table" | "blockquote" | "thematic_break"
                                                    -- | "transclusion"
    parent_uuid     TEXT REFERENCES nodes(uuid),    -- NULL for root
    position        REAL NOT NULL,                  -- gap-based float ordering (1000, 2000, ...)
                                                                                
    -- Content addressing
    has_content     INTEGER NOT NULL DEFAULT 0,     -- 1 = content file exists, 0 = derived (sentence/container)
    content_path    TEXT,                           -- relative path: "{first2}/{uuid}.json" (NULL if no file)
    plain_text      TEXT NOT NULL DEFAULT '',       -- extracted plain text for FTS + sentence segmentation
    structural_hash TEXT NOT NULL,                  -- SHA-256 of normalized plain_text (re-ingest diff)

    -- Sentence nodes only
    char_start      INTEGER,                        -- byte offset into parent paragraph plain_text
    char_end        INTEGER,                        -- byte offset into parent paragraph plain_text

    -- Heading hierarchy (from SiSU's lev0-lev7 pattern)
    heading_level   INTEGER,                        -- 1-6 for heading nodes, NULL for non-headings
    section_path    TEXT,                           -- dot-separated heading numbers: "1.2.3"

    -- Versioning
    version         INTEGER NOT NULL DEFAULT 1,
    status          TEXT NOT NULL DEFAULT 'active', -- "active" | "deleted"
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);

CREATE UNIQUE INDEX idx_nodes_uuid ON nodes(uuid);
CREATE INDEX idx_nodes_doc_id ON nodes(doc_id);
CREATE INDEX idx_nodes_parent ON nodes(parent_uuid);
CREATE INDEX idx_nodes_position ON nodes(doc_id, position);
CREATE INDEX idx_nodes_hash ON nodes(doc_id, structural_hash);
CREATE INDEX idx_nodes_type ON nodes(node_type);
CREATE INDEX idx_nodes_status ON nodes(status);

-- ── FTS5: Full-text search (content-sync with nodes table) ────

-- Content-sync FTS5: the FTS table is just an index.
-- It uses rowid to reference nodes.id (the integer PK).
-- This avoids duplicating text — the plain_text lives in nodes.
CREATE VIRTUAL TABLE nodes_fts USING fts5(
    uuid            UNINDEXED,      -- stored for retrieval, not searchable
    node_type       UNINDEXED,      -- stored for filtering
    doc_id          UNINDEXED,      -- stored for scoping
    plain_text,                     -- THE searchable content
    content=nodes,                  -- sync with nodes table
    content_rowid=id,              -- nodes.id is the integer PK rowid
    tokenize='porter unicode61'     -- English stemming + Unicode
);

-- FTS5 sync triggers (3 per table — INSERT, DELETE, UPDATE)

-- AFTER INSERT: sync new rows to FTS index
CREATE TRIGGER nodes_ai AFTER INSERT ON nodes BEGIN
    INSERT INTO nodes_fts(rowid, uuid, node_type, doc_id, plain_text)
    VALUES (new.id, new.uuid, new.node_type, new.doc_id, new.plain_text);
END;

-- AFTER DELETE: remove deleted rows from FTS index
CREATE TRIGGER nodes_ad AFTER DELETE ON nodes BEGIN
    INSERT INTO nodes_fts(nodes_fts, rowid, uuid, node_type, doc_id, plain_text)
    VALUES ('delete', old.id, old.uuid, old.node_type, old.doc_id, old.plain_text);
END;

-- AFTER UPDATE of plain_text: delete old entry, insert new
CREATE TRIGGER nodes_au AFTER UPDATE OF plain_text ON nodes BEGIN
    INSERT INTO nodes_fts(nodes_fts, rowid, uuid, node_type, doc_id, plain_text)
    VALUES ('delete', old.id, old.uuid, old.node_type, old.doc_id, old.plain_text);
    INSERT INTO nodes_fts(rowid, uuid, node_type, doc_id, plain_text)
    VALUES (new.id, new.uuid, new.node_type, new.doc_id, new.plain_text);
END;

-- ── W3C Web Annotations ────────────────────────────────────────

-- Annotations are stored as W3C JSON-LD blobs with denormalized columns
-- for indexing and status tracking.
CREATE TABLE annotations (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    uuid            TEXT NOT NULL UNIQUE,           -- UUID v7
    annotation      TEXT NOT NULL,                  -- Full W3C Web Annotation JSON-LD blob
    target_uuid     TEXT NOT NULL,                  -- denormalized: the sentence/block UUID
    target_doc_id   TEXT NOT NULL,                  -- denormalized: the document UUID
    motivation      TEXT NOT NULL DEFAULT 'commenting',  -- "commenting" | "highlighting" | "tagging" | "linking"
    status          TEXT NOT NULL DEFAULT 'active', -- "active" | "active_partial" | "orphan" | "deleted"
                                                    -- Extension to W3C spec for anchoring state
    creator         TEXT,                           -- agent identifier (e.g., "agent:web-researcher")
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);

CREATE UNIQUE INDEX idx_annotations_uuid ON annotations(uuid);
CREATE INDEX idx_annotations_target ON annotations(target_uuid);
CREATE INDEX idx_annotations_doc ON annotations(target_doc_id);
CREATE INDEX idx_annotations_status ON annotations(status);

-- ── Transclusion Edges ─────────────────────────────────────────

CREATE TABLE transclusions (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    uuid                TEXT NOT NULL UNIQUE,           -- UUID v7
    predicate           TEXT NOT NULL,                  -- "transcludes" | "cites" | "derives-from"
                                                        -- | "responds-to" | "supports" | "contradicts"
                                                        -- | "supersedes" | "exemplifies"
    source_node_uuid    TEXT NOT NULL REFERENCES nodes(uuid),
    source_doc_uuid     TEXT NOT NULL REFERENCES documents(uuid),
    target_node_uuid    TEXT NOT NULL REFERENCES nodes(uuid),
    target_doc_uuid     TEXT NOT NULL REFERENCES documents(uuid),
    version_at_include  INTEGER NOT NULL,               -- target node version when edge was created
    status              TEXT NOT NULL DEFAULT 'live',    -- "live" | "stale" | "orphaned"
    created_at          TEXT NOT NULL
);

CREATE UNIQUE INDEX idx_transclusions_uuid ON transclusions(uuid);
CREATE INDEX idx_transclusions_source ON transclusions(source_node_uuid);
CREATE INDEX idx_transclusions_target ON transclusions(target_node_uuid);
CREATE INDEX idx_transclusions_status ON transclusions(status);

-- ── Provenance Activities (append-only, never updated) ────────

CREATE TABLE activities (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    uuid            TEXT NOT NULL UNIQUE,           -- UUID v7
    activity_type   TEXT NOT NULL,                  -- "ingest" | "reingest" | "annotate" | "transclude"
                                                    -- | "delete" | "project" | "search"
    input_ids       TEXT,                           -- JSON array of consumed UUIDs
    output_ids      TEXT,                           -- JSON array of produced UUIDs
    agent           TEXT,                           -- who performed the activity
    config          TEXT,                           -- JSON: parameters (format, parser version, etc.)
    started_at      TEXT NOT NULL,
    ended_at        TEXT
);

CREATE UNIQUE INDEX idx_activities_uuid ON activities(uuid);
CREATE INDEX idx_activities_type ON activities(activity_type);

-- ── Session Marker Map (§N → UUID, session-local, ephemeral) ──

-- Not a persistent table. The marker map lives in memory during
-- a projection session. When an agent calls read(doc_id),
-- the runtime assigns §1, §2, ... §N to each sentence in the output
-- and returns a map: { 1: uuid, 2: uuid, ..., N: uuid }.
-- The agent then annotates using §N + optional bounded text.
```

## FTS5 Queries (Production Patterns)

### Search with BM25 ranking

```sql
SELECT n.uuid, n.node_type, n.plain_text,
       highlight(nodes_fts, 3, '<mark>', '</mark>') AS snippet,
       bm25(nodes_fts, 0.0, 0.0, 0.0, 1.0) AS score
FROM nodes_fts
JOIN nodes n ON nodes_fts.rowid = n.id
WHERE nodes_fts MATCH ?
  AND n.doc_id = ?
  AND n.status = 'active'
ORDER BY bm25(nodes_fts)
LIMIT ?
```

### Query sanitization (mandatory — raw user input crashes FTS5)

```rust
fn sanitize_fts_query(query: &str) -> String {
    // Wrap in quotes and escape internal quotes to prevent syntax errors
    let escaped = query.replace('"', "\"\"");
    format!("\"{}\"", escaped)
}
```

## Pragmas (from rusqlite production patterns)

```rust
conn.execute_batch("
    PRAGMA journal_mode = WAL;
    PRAGMA foreign_keys = ON;
    PRAGMA synchronous = NORMAL;
    PRAGMA temp_store = MEMORY;
    PRAGMA mmap_size = 30000000000;  -- 30GB mmap for large DBs
    PRAGMA page_size = 4096;
")?;
```

## Byte Offsets vs Character Offsets

`icu_segmenter` returns **byte offsets** (UTF-8). The `char_start` and `char_end` columns in the `nodes` table also store **byte offsets** — not Unicode character offsets. This is because:

1. `plain_text` is UTF-8 Rust `String`, and slicing by byte offset is O(1)
2. Pandoc inline positions are also byte-based
3. The FTS5 `highlight()` function operates on byte positions

If Unicode character offsets are ever needed (for a UI that counts by visible characters), they can be computed on the fly: `text[..byte_pos].chars().count()`.
