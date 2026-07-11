# Text Runtime — Storage Architecture

**Date:** 2026-07-09 (Updated: 2026-07-11)
**Status:** Implemented (Phase 1 Engine + Phase 2 Daemon)
**Scope:** Content storage, annotation model, ingestion pipeline, agent interface
**Supersedes:** Portions of `decisions.md` related to storage

---

## 1. Core Principle

The runtime owns the text. Source files — Markdown, TXT, HTML, LaTeX, transcripts — are import channels and projection surfaces. Once text enters the runtime, the runtime's copy is canonical. The source file being deleted, moved, or renamed has no effect on the runtime's integrity.

This is a fundamental inversion from the original design, which treated source files as authoritative and the object store as a derived index. That model creates fragility: every reference chain depends on a file path that the human or agent can change at any moment.

The corrected model:

```
Source file (any format)
        ↓ ingest (once)
Runtime content store (canonical)
        ↓ project (any time)
Output (Markdown, HTML, PDF, LaTeX, Djot, ...)
```

The source file is provenance metadata after ingest. Nothing else.

---

## 2. Two-Store Architecture

The single most important structural decision: **text never goes in SQL**.

SQL is the right tool for structured, relational, queryable data — identities, types, parent-child links, annotation state, transclusion edges, activity history. It is the wrong tool for raw text content. Storing paragraphs and sentences as TEXT columns in a relational database degrades query performance through B-tree overflow pages, mixes content concerns with structural concerns, and fights the design of the tool.

Text content lives in a separate store. SQL stores everything else.

```
┌─────────────────────────────────────────────────────────┐
│  .textruntime/                                          │
│                                                         │
│  db.sqlite          ← structure, identity, relations    │
│                        NO text content here             │
│                                                         │
│  content/           ← text content, one file per node   │
│    {00-ff}/            UUID-named, directory fanout     │
│      {uuid}.json       Pandoc AST fragment              │
│                                                         │
│  tmp/               ← atomic write staging              │
│  config.json        ← runtime version and settings      │
└─────────────────────────────────────────────────────────┘
```

---

## 3. The Content Store

### Physical layout

Files in a runtime-managed hidden directory. The agent and the human never interact with this directory directly. The runtime is the only writer.

```
.textruntime/content/
  00/
  01/
    8f2c{remaining-uuid-chars}.json   ← one node's text
    9a44{remaining-uuid-chars}.json
  02/
  ...
  ff/
```

Directory fanout by first two hex characters of the UUID. 256 subdirectories. At 50,000 nodes: ~195 files per directory. Filesystem metadata stays manageable.

### UUID as the address

The filename IS the address. To retrieve a node's text: read `.textruntime/content/{first2}/{uuid}.json`. No secondary index needed beyond SQLite.

### Atomic write protocol

Every write goes through:
1. Write to `.textruntime/tmp/{uuid}.json.tmp`
2. `fsync`
3. `rename` to `.textruntime/content/{first2}/{uuid}.json`

`rename` is atomic on all POSIX filesystems. Readers never see partial writes.

### Scale and upgrade path

The filesystem content store is the starting point. It is abstracted behind a one-method interface:

```
get(uuid) → bytes
put(uuid, bytes)
delete(uuid)
```

If node count exceeds filesystem comfort (~500K nodes), the backend swaps to LMDB — a battle-tested embedded key-value store used by Firefox IndexedDB, OpenLDAP, and Tor. Same interface. Nothing else changes.

---

## 4. What Goes in the Content Store

### Composition, not duplication

Sentences are not a Pandoc AST construct. In Pandoc's model, a paragraph is a `Para` block containing a flat list of `Inline` elements. Sentences are segmentation we apply on top of that inline list — they are slices of the paragraph's inline array, not independent blocks.

Because sentences are structurally contained within their parent paragraph, they do not get content files. Their text is derived by reading the parent paragraph file and slicing by character offsets stored in SQLite. This eliminates redundancy and the consistency problem it creates: changing a sentence means overwriting one paragraph file and updating two integers in SQLite — no cascade, no duplication.

The rule: **block-level nodes that carry independent text get content files. Everything else is derived.**

| Node type | Content file? | Text source |
|---|---|---|
| `sentence` | ❌ | parent paragraph file, sliced by `char_start`/`char_end` |
| `paragraph` | ✅ | Pandoc Para block |
| `heading` | ✅ | Pandoc Header block with level |
| `code_block` | ✅ | Pandoc CodeBlock with language |
| `list_item` | ✅ | Pandoc list item block |
| `table` | ✅ | Pandoc Table block |
| `blockquote` | ✅ | Pandoc BlockQuote block |
| `section` | ❌ | assembled from children on demand |
| `document` | ❌ | assembled from children on demand |

The `has_content` flag in the nodes table is the indicator. A reader never attempts a file lookup when `has_content = 0`.

### Reading a sentence

```
"give me sentence uuid abc123"
1. SELECT parent_uuid, char_start, char_end FROM nodes WHERE uuid = 'abc123'
   → parent = paragraph p1, chars 0–52
2. read content/{fanout}/p1.json → paragraph Pandoc AST
3. extract plain text from Para inline array
4. slice plain text [0:52]
→ "The activation energy of this reaction is 5.2 kJ/mol."
```

One file read (the parent paragraph). The sentence UUID is still a first-class stable identity in SQLite — it just does not need its own file.

---

## 5. Content File Format

### File format: `.json`

Agents produce and consume JSON natively. No parsing step. Every language reads and writes JSON.

### Text format: Pandoc AST JSON

The text content inside each file is a Pandoc AST fragment — the single Block element or list of Inline elements that represents this node's content.

**Why Pandoc AST:**
- 20+ years in production
- Format-agnostic by design — the same representation whether source was Markdown, LaTeX, HTML, or Djot
- 40+ output formats via Pandoc's existing writers
- Every block type and inline type maps directly to the runtime's structural hierarchy
- Well-defined, versioned JSON serialization

**Human readability is a projection property, not a storage property.** The content store holds the canonical representation. When a human or agent needs to read a document, the runtime projects the assembled AST through Pandoc's writers to any format. A 100-line Markdown file is a 100-line Markdown file when projected — regardless of how it's stored internally.

### Examples
### Examples

Paragraph (`content/01/9a44....json`) — contains all its sentences inline:
```json
{"t": "Para", "c": [
  {"t": "Str", "c": "The activation energy of "},
  {"t": "Strong", "c": [{"t": "Str", "c": "this reaction"}]},
  {"t": "Str", "c": " is 5.2 kJ/mol. "},
  {"t": "Str", "c": "Temperature plays a critical role."},
  {"t": "Str", "c": " This suggests further study."}
]}
```

Sentences are not stored as separate files. They are SQLite rows pointing into this Para block via `char_start`/`char_end`:
```
sentence 1: char_start=0,  char_end=52  → "The activation energy of this reaction is 5.2 kJ/mol."
sentence 2: char_start=53, char_end=92  → "Temperature plays a critical role."
sentence 3: char_start=93, char_end=121 → "This suggests further study."
```

Heading level 2 (`content/02/ab34....json`):
```json
{"t": "Header", "c": [2, ["", [], []], [{"t": "Str", "c": "Experimental Methods"}]]}
```

Code block (`content/04/cd56....json`):
```json
{"t": "CodeBlock", "c": [["", ["python"], []], "print('hello')"]}
```
---

## 6. The SQLite Layer

SQLite holds everything structural. No text content. The `nodes` table stores the address of each node's content file — via `has_content` and the UUID itself — not the content.

### Schema

```sql
documents
  uuid          TEXT PRIMARY KEY
  title         TEXT
  import_format TEXT          -- "markdown" | "txt" | "html" | "latex" | ...
  import_path   TEXT NULL     -- provenance only. where it came from.
  import_hash   TEXT NULL     -- sha256 of source at last ingest. change detection.
  root_node_uuid TEXT
  ingested_at   TEXT
  version       INTEGER DEFAULT 1

nodes
  uuid          TEXT PRIMARY KEY
  type          TEXT          -- "sentence" | "paragraph" | "heading" | "section"
                              -- | "code_block" | "list_item" | "table" | "document" | ...
  doc_id        TEXT REFERENCES documents(uuid)
  parent_uuid   TEXT REFERENCES nodes(uuid)
  position      REAL          -- gap-based float ordering (1000, 2000, ...)
  structural_hash TEXT        -- sha256 of normalized plain text. re-ingest diff.
  has_content   INTEGER       -- 1 = content file exists. 0 = no file (sentence or container).
  char_start    INTEGER NULL  -- sentence only: offset in parent paragraph plain text
  char_end      INTEGER NULL  -- sentence only: offset in parent paragraph plain text
  version       INTEGER DEFAULT 1
  status        TEXT DEFAULT 'active'  -- "active" | "deleted"
  created_at    TEXT
  updated_at    TEXT

-- W3C Web Annotation (see Section 7)
annotations
  uuid          TEXT PRIMARY KEY
  annotation    TEXT          -- full W3C JSON-LD blob
  target_uuid   TEXT          -- denormalized from annotation JSON for indexing
  motivation    TEXT          -- denormalized for querying
  status        TEXT          -- "active" | "active_partial" | "orphan" | "deleted"
                              -- extension to W3C spec. not in the standard.
  created_at    TEXT
  updated_at    TEXT

transclusions
  uuid          TEXT PRIMARY KEY
  predicate     TEXT          -- "transcludes" | "cites" | "derives-from"
                              -- | "responds-to" | "supports" | "contradicts"
                              -- | "supersedes" | "exemplifies"
  source_node_uuid  TEXT REFERENCES nodes(uuid)
  source_doc_uuid   TEXT REFERENCES documents(uuid)
  target_node_uuid  TEXT REFERENCES nodes(uuid)
  target_doc_uuid   TEXT REFERENCES documents(uuid)
  version_at_include INTEGER  -- target node version when edge was created
  status        TEXT          -- "live" | "stale" | "orphaned"
  created_at    TEXT

activities                    -- append-only. never updated.
  uuid          TEXT PRIMARY KEY
  type          TEXT          -- "ingest" | "reingest" | "annotate"
                              -- | "transclude" | "delete" | "project"
  input_ids     TEXT          -- JSON array of UUIDs consumed
  output_ids    TEXT          -- JSON array of UUIDs produced
  agent         TEXT
  started_at    TEXT
  ended_at      TEXT
  config        TEXT          -- JSON: parameters (format, parser version, etc.)

-- Derived. Fully rebuildable from content files.
nodes_fts (FTS5 virtual table)
  uuid
  type
  doc_id
  plain_text    -- all Str inlines concatenated from Pandoc AST
```

**The activities table is the only table that is never updated.** Everything else is mutable. Activities are events — past tense. Append only.

---

## 7. Annotation Layer: W3C Web Annotation

### Decision

Adopt the W3C Web Annotation Data Model in full for all annotations. Do not build a custom annotation schema. The W3C spec has handled every selection edge case across 20+ years of production use. There is no reason to re-solve problems that are already solved.

### Why W3C

The W3C Web Annotation spec defines a complete selector vocabulary:

| Selector | What it handles |
|---|---|
| `TextPositionSelector` | Character offsets — fast path when nothing changed |
| `TextQuoteSelector` | Exact text + prefix/suffix — recovery path after edits |
| `RangeSelector` | Selection spanning across node boundaries |
| `FragmentSelector` | Scope to a specific UUID, then select within it |
| `refinedBy` | Nested selectors — scope first, select precisely inside |

Every case is covered. Cross-sentence highlights, cross-paragraph highlights, single words, ambiguous text that appears twice, selections spanning structural boundaries. All handled.

### The one adaptation

W3C was designed for web resources identified by URLs. The runtime uses UUIDs. The swap:

```
W3C:          "source": "https://example.com/document"
Text Runtime: "source": "urn:uuid:019d3f..."
```

That is the only structural change. Everything else in the spec applies directly.

### What a stored annotation looks like

```json
{
  "@context": "http://www.w3.org/ns/anno.jsonld",
  "@type": "Annotation",
  "motivation": "highlighting",
  "target": {
    "source": "urn:uuid:019d3f...",
    "selector": [
      {
        "type": "TextPositionSelector",
        "start": 24,
        "end": 37
      },
      {
        "type": "TextQuoteSelector",
        "exact": "this reaction",
        "prefix": "activation energy of ",
        "suffix": " is 5.2"
      }
    ]
  },
  "body": {
    "type": "TextualBody",
    "value": "needs citation",
    "format": "text/plain"
  }
}
```

This JSON-LD blob is stored in `annotations.annotation`. The columns `target_uuid`, `motivation`, and `status` are denormalized for fast SQL queries.

### Selector library

**Apache Annotator** (`@apache-annotator/selector`) — Apache Software Foundation, JavaScript/TypeScript, npm.

Implements the full W3C selector vocabulary: TextQuoteSelector creation and resolution, TextPositionSelector, RangeSelector, refinedBy nesting. The runtime does not write selector logic. It imports this library and calls it.

### The status state machine

W3C does not define an annotation lifecycle. This is a runtime extension:

```
active          → annotation has a live, verified anchor
active_partial  → some target nodes were deleted; anchor partially survives
orphan          → anchor is completely gone; text no longer exists or restructured
deleted         → explicitly deleted by agent
```

`orphan` is NOT deleted. An orphaned annotation retains its payload and its last-known selectors. The text may be restored. The annotation itself has meaning. It surfaces as "source deleted or changed beyond recognition" — not silently removed.

### How an agent annotates

The agent never handles positions, offsets, or UUIDs directly for annotation. The interface is:

```
annotate({
  type:    "highlighting",
  quote:   "this reaction",
  prefix:  "activation energy of",    ← disambiguation if needed
  suffix:  "is 5.2",
  payload: { note: "needs citation", color: "yellow" }
})
```

The runtime resolves the quote to a UUID + character offsets, builds both selectors via Apache Annotator, and stores the W3C-compliant annotation in SQLite. The agent never saw a position. Never saw a UUID. Never saw the AST.

---

## 8. Ingestion Pipeline

### Two entry paths

**Path 1 — Direct API (primary):**
```
runtime.ingest(text: string, format: string, metadata: {title, ...})
```
Agent calls this directly. No file involved. Text enters the runtime cleanly.

**Path 2 — File watcher daemon (secondary):**
```
daemon watches configured directories
new/modified file → runtime.ingest(file_path)
```
Handles the existing situation: thousands of Markdown files lying around. Source file is consumed into the runtime. The file remains untouched but is no longer the source of truth.

Both paths converge on the same pipeline. After ingest, the source file (if any) is just `documents.import_path` — provenance metadata.

### Pipeline stages

```
Source (text string or file)
        ↓
Format Normalizer
  detect format, normalize unicode, normalize line endings

        ↓
Pandoc Parser
  parse to full Pandoc AST via Pandoc CLI or library binding

        ↓
Structural Segmenter
  walk Pandoc AST
  identify: document → sections → paragraphs → sentences
  assign structural position to each node

        ↓
UUID Assigner
  compute structural_hash per node (sha256 of normalized plain text)
  query SQLite for existing hash matches (re-ingest diff — see Section 9)
  assign: keep existing UUID or generate new UUID v7

Content Writer
  for each block node with has_content = 1:
    write tmp/{uuid}.json.tmp  (Pandoc block AST)
    fsync
    rename → content/{fanout}/{uuid}.json
  sentence nodes: no file write — char_start/char_end stored in SQLite only

        ↓
SQLite Writer (single transaction)
  INSERT or UPDATE documents row
  INSERT or UPDATE nodes rows
  UPDATE deleted nodes: status = 'deleted'
  UPDATE orphaned annotations: status = 'orphan'
  UPDATE orphaned transclusions: status = 'orphaned'

        ↓
FTS Indexer
  for each new/updated node:
    extract plain text from Pandoc AST (concatenate all Str inlines)
    INSERT or UPDATE nodes_fts

        ↓
Activity Logger
  INSERT activity: {type:"ingest", input_ids, output_ids, agent, timestamps}
```

---

## 9. Re-ingestion Protocol

When a document is updated (agent re-pushes or file watcher detects a change):

```
1. Parse new source → full Pandoc AST
2. Segment into nodes
3. For each new node:
   a. Compute structural_hash
   b. Look up hash in SQLite WHERE doc_id = this doc

      Exact hash match
      → Keep UUID, skip content file write (nothing changed)

      Fuzzy match (edit distance ≤ 20% of node length)
      → Keep UUID, overwrite content file, increment version,
        update structural_hash, update updated_at

      No match
      → New UUID v7, new content file, new SQLite row

4. For old nodes not matched by any new node:
   → UPDATE nodes SET status = 'deleted'
   → Content file stays on disk
     (orphaned annotations still reference the UUID)
   → Annotations targeting deleted nodes:
      UPDATE annotations SET status = 'orphan'
   → Transclusions pointing to deleted nodes:
      UPDATE transclusions SET status = 'orphaned'

5. INSERT activity: {type:"reingest", input_ids:[...old...], output_ids:[...new...]}
```

The fuzzy match threshold (20% edit distance) is configurable in `config.json`.

---

## 10. Version Preservation

**Decision: overwrite in place. Activity log is the history.**

Every content file stores current state only. When a node is updated on re-ingest, the content file is overwritten with the new Pandoc AST. The `version` counter in SQLite increments. The activity record captures what changed and when.

Full text history is not stored by default. If full versioning is required: `git init .textruntime/`. Every content file change is tracked automatically by git at no extra design cost. The activity log and git log together give complete history.

---

## 11. Agent Interface

### The cursor problem

A human annotates by pointing — moving a cursor to a position, selecting text, and the system resolves that selection to character offsets, prefix/suffix, and structural identity. The human never calculates positions. The system does.

An agent has no cursor. So what is its equivalent?

**Rejected approaches:**
- **Inline UUIDs in projected text** — UUID v7 is 36 characters. A 50-sentence document would add ~1800 characters of UUID noise. Catastrophic for token efficiency and attention.
- **Agent calculates prefix/suffix** — LLMs hallucinate positions and boundaries. Semiont (production W3C annotation system, The AI Alliance) found that LLMs cannot reliably supply character offsets. Their system computes offsets from quoted text, treating LLM positions as unreliable hints.
- **Agent provides character offsets or column numbers** — LLMs cannot count characters reliably. This is a known failure mode across all coding agents.

**The solution: two-step protocol — §N markers for display, UUIDs for annotation.**

### Reading (with marker_map)

When the runtime projects a document for an agent, it injects lightweight sentence markers into the rendered text and returns a `marker_map`:

```
read(doc_id, format: "markdown", markers: true)
→ {
    text: "§1  The quantum entanglement hypothesis...\n§2  Einstein...",
    format: "markdown",
    marker_map: {
      "1": "019f4a7b-d123-4567-8901-234567890abc",
      "2": "019f4a7b-e456-7890-1234-567890abcdef"
    }
  }
```

The projected text with markers:
```
§1  The quantum entanglement hypothesis has been debated for decades.
§2  Einstein called it "spooky action at a distance."
§3  Modern experiments have conclusively demonstrated the phenomenon.

§4  The implications for computing are profound.
§5  Quantum computers exploit entanglement for parallel computation.
```

Token cost: ~3 characters per sentence (§ + number + space). For a 200-sentence document: ~600 characters of markers. Compare to UUIDs at the same granularity: ~7200 characters.

The markers are **ephemeral** — they change when the document is edited, just like line numbers change when code is edited. The marker_map is returned to the client, which resolves §N → UUID before calling annotate. The daemon stores no per-connection §N state.

This pattern is production-validated:
- **Ihsaan Patel (2025)**: "Inject markers into the document that allow the LLM to reference specific portions of the text... This eliminates the subtle hallucination problem."
- **DeepRead (arxiv, 2025)**: Coordinates `(doc_id, section_id, paragraph_index)` for agent navigation
- **Tensorlake Citation-Aware RAG**: Inline `[2.1]` anchors, LLM outputs anchor IDs, system resolves
- **Hashline Protocol (pi-coding-agent ecosystem)**: `LINE#HASH` anchors for code editing — same principle at line granularity

The agent receives clean projected text with sentence markers. Never the Pandoc AST. Everything else (UUIDs, parent-child links, content hashes, character offsets) is hidden from the agent.

### Annotating (by UUID)

The agent annotates by UUID, not by sentence number. The client resolves §N → UUID using the marker_map, then sends the UUID to the annotate endpoint.

**Whole sentence (most common):**
```json
{
  "workspace": "notes",
  "doc_id": "019f4a1b-xxxx",
  "sentence_uuid": "019f4a7b-d123-4567-8901-234567890abc",
  "motivation": "highlighting"
}
```

**Sub-sentence (bounded text quoting):**
```json
{
  "workspace": "notes",
  "doc_id": "019f4a1b-xxxx",
  "sentence_uuid": "019f4a7b-e456-7890-1234-567890abcdef",
  "quote": "spooky action at a distance",
  "motivation": "citation"
}
```

The agent **never** provides:
- UUIDs inline in projected text (too long, wasteful tokens)
- Prefix/suffix for disambiguation (non-deterministic — the runtime handles this)
- Character offsets (LLMs can't count)
- Column positions (same problem)
- Sentence numbers to the annotate endpoint (marker_map resolution is client-side)

The agent **only** provides:
- A sentence UUID (from the marker_map — stable, deterministic)
- Optionally a quoted phrase within that sentence (for sub-sentence precision)

### Resolution chain

When the client calls `annotate({ sentence_uuid: "019f4a7b-e456...", quote: "spooky action" })`:

```
1. Runtime receives the sentence UUID directly (resolved client-side from marker_map)
2. Runtime reads sentence text from content store
3. Runtime searches for "spooky action" WITHIN that one sentence
   (bounded search — near-zero ambiguity in 10-30 word scope)
4. Runtime calculates char_start/char_end within the sentence
5. Runtime builds W3C selectors via Apache Annotator:
   - TextPositionSelector: {start: 4, end: 25}
   - TextQuoteSelector: {exact: "spooky action", prefix: "Einstein called it \"", suffix: " at a distance\""}
6. Runtime stores full W3C JSON-LD annotation blob in SQLite
7. Returns annotation UUID to agent
```

The critical insight: **bounded text quoting**. This confines the quote search to ONE KNOWN SENTENCE — making sub-sentence quoting nearly unambiguous without needing prefix/suffix from the agent.

### Why this works where quote-from-document fails

| Approach | Search scope | Ambiguity risk | Agent reliability | Daemon state |
|---|---|---|---|---|
| Quote from full document (Semiont) | Entire document | High — phrase may appear twice | Medium — LLM may hallucinate differences | N/A |
| Sentence UUID + optional quote | One sentence (~10-30 words) | Near-zero — phrases rarely repeat | High — UUID is deterministic, quote is bounded | Stateless |

### Navigation

Pure SQLite. No content file reads for structural navigation.

```
parent(uuid)    → SELECT parent_uuid FROM nodes WHERE uuid = ?
children(uuid)  → SELECT uuid FROM nodes WHERE parent_uuid = ? ORDER BY position
siblings(uuid)  → SELECT uuid FROM nodes WHERE parent_uuid = (parent of uuid) ORDER BY position
prev(uuid)      → child immediately before in parent's children, by position
next(uuid)      → child immediately after in parent's children, by position
toc(doc_id)     → SELECT uuid, type FROM nodes WHERE doc_id = ? AND type = 'heading' ORDER BY position
```

Note: agents navigate by UUID when they already have one (from a previous annotation, a transclusion edge, or a search result). The sentence markers are for READING and ANNOTATING — the two operations where the agent looks at projected text. Navigation operates on UUIDs received from prior interactions with the runtime.

### Loading a single node

```
load(uuid)
→ SQLite lookup: type, parent_uuid, position, doc_id, has_content,
                 char_start, char_end, status

→ if has_content = 1:
    read content/{fanout}/{uuid}.json → Pandoc AST block
    project to plain text

→ if type = 'sentence' (has_content = 0):
    read content/{fanout}/{parent_uuid}.json → parent paragraph AST
    extract plain text from Para inline array
    slice plain text [char_start : char_end]

→ if type = 'section' or 'document' (has_content = 0):
    walk children via SQLite, assemble from block children on demand
```

### Projecting

```
project(doc_id, format, markers: true)
→ walk SQLite tree: ordered node UUIDs for this doc
→ read content files for all has_content = 1 nodes
→ assemble full Pandoc AST (ordered Block array)
→ if markers = true:
    inject §N at each sentence boundary (build number→UUID mapping)
→ pass to Pandoc writer → Markdown, HTML, PDF, LaTeX, Djot, EPUB, ...
→ return { text, marker_map: {1: uuid1, 2: uuid2, ...} }
```

The `marker_map` is ephemeral — it lives for the duration of the agent's read session. It is not persisted.


---

## 12. Transclusion Edges

A transclusion is a typed directed edge between nodes across documents. Not embedded content — a live reference.

```sql
INSERT INTO transclusions VALUES (
  uuid          = new UUID v7,
  predicate     = 'cites',           -- or: transcludes | derives-from | responds-to
                                     --     supports | contradicts | supersedes | exemplifies
  source_node_uuid = 'abc123',
  source_doc_uuid  = 'docA',
  target_node_uuid = 'def456',
  target_doc_uuid  = 'docB',
  version_at_include = 3,            -- target node version at time of edge creation
  status        = 'live'
)
```

**Staleness detection:**
```sql
SELECT t.uuid, t.version_at_include, n.version
FROM transclusions t
JOIN nodes n ON t.target_node_uuid = n.uuid
WHERE t.version_at_include < n.version
-- returns transclusions pointing at nodes that have since changed
```

---

## 13. What Already Exists

The following open-source projects were surveyed. None assembles this combination:

**AppFlowy** (60K+ stars) — closest in architecture: SQLite for metadata, RocksDB for content (CRDT binary state). But content is Quill Delta format (not Pandoc AST), no W3C annotations, no transclusion model, no provenance, built for human editors not agents.

**Cept** — Notion clone backed by Git files. Validates the content-in-files approach. But files are plain Markdown, no structured content model.

**Giraffle** — canonical block AST in PostgreSQL, Markdown as derived export. Close in philosophy (canonical internal, project to format) but PostgreSQL stores text, no agent surface.

The gap that remains: no open-source project provides a local-first, agent-native document runtime with format-agnostic content storage (Pandoc AST), W3C Web Annotation selectors, typed transclusion edges, per-node provenance, and clean separation of text from SQL.

---

## 14. What This Is Not

- **Not an editor.** The runtime has no UI. Any editor (human or agent) writes text and calls the ingest API or drops files in a watched directory.
- **Not a search engine.** The primary operation is navigation — following a known UUID to its content. FTS5 exists for cases where a UUID is not already known, but it is not the primary interface.
- **Not a collaboration platform.** No OT, no CRDT, no real-time sync. Local-first, single-writer.
- **Not a new format.** Pandoc AST is 20 years old. W3C Web Annotation is a W3C standard. No format was invented here.

---

## 15. Summary

```
Source (any format)
        ↓ ingest
        
.textruntime/
  db.sqlite                     ← identity, structure, annotations (W3C), 
                                   transclusions, activities, FTS index
                                   NO TEXT HERE
  
  content/{fanout}/{uuid}.json  ← Pandoc AST fragment per node
                                   format-agnostic
                                   runtime-owned, not user-owned
                                   projects to any format via Pandoc
                                   
        ↓ project (with §N sentence markers for agents)
        
Any format (Markdown, HTML, PDF, LaTeX, Djot, ...)
```

Two stores. Two jobs. Text never in SQL. Pandoc AST as the canonical content format. W3C Web Annotation for all annotations. Sentence markers (§N) as the agent's cursor — deterministic, token-efficient, system-resolved to UUIDs and W3C selectors. Agent reads clean projections, annotates by sentence number + optional bounded quote, navigates by UUID. Source files are import channels, not sources of truth.




