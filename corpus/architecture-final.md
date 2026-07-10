# Text Runtime — Final Architecture

**Date**: 2026-07-10
**Status**: Settled — this is what was built

---

## The Principle

Text is treated as a stream of characters everywhere. The Text Runtime treats it as a collection of addressable, stable objects.

Two human activities — writing and reading — remain pure and uninterrupted. The runtime operates between them, invisibly. The human never sees UUIDs, never manages IDs, never knows the runtime exists.

The agent is the primary consumer of the runtime's structured representation.

---

## The Two Modes

```
WRITE  →  [text file — Markdown, TXT, transcript, anything]
                    ↓
              INGESTION PIPELINE        ← runtime activates here
                    ↓
              OBJECT STORE              ← stable, addressable tree
                    ↓
              [annotation / reference / query / transclusion]
                    ↓
READ   ←  [clean text projected back — UUIDs never visible]
```

---

## The Ingestion Pipeline (6 Stages)

```
Stage 1: FORMAT NORMALIZER
  In:  raw bytes or string (any format)
  Out: normalized unicode string + detected format tag
  Do:  NFC unicode, strip invisible chars, normalize line endings,
       detect format (markdown / plaintext / transcript / html / ...)

Stage 2: STRUCTURAL PARSER (via Pandoc)
  In:  normalized string + format tag
  Out: Pandoc AST JSON — typed nodes, NO IDs yet
  Do:  pandoc-server /batch endpoint
       output: Document → Section → Paragraph, strictly typed
       failure mode: partial tree on malformed input, never crash

Stage 3: SENTENCE SEGMENTER
  In:  Pandoc AST (paragraph nodes have Inlines, no children)
  Out: Pandoc AST with sentence boundary offsets per paragraph
  Do:  icu_segmenter (Unicode Consortium) on plain text of each paragraph
       store as char_start/char_end offsets into paragraph plain text
       code blocks, tables, list items → atomic (no sentence split)

Stage 4: UUID ASSIGNER
  In:  raw tree (no IDs)
  Out: identity tree (every node has UUID v7)
  Do:  first ingestion → mint UUID v7 for every node
       re-ingestion → identity matching (see Re-Ingestion below)
       guarantee: every node has exactly one stable UUID

Stage 5: CONTENT WRITER + SQLITE WRITER
  In:  identity tree
  Out: content files + SQLite records
  Do:  block nodes → .textruntime/content/{00-ff}/{uuid}.json (atomic write)
       all nodes → SQLite in single transaction
       sentence nodes → SQLite rows only (no content files)

Stage 6: INDEX BUILDER + ACTIVITY LOGGER
  In:  persisted records
  Out: FTS5 index, activity log entry
  Do:  FTS5 content-sync update
       append Activity record (type: parse, inputs: source_path, outputs: doc_uuid)
```

---

## The Object Store — Core Data Model

### NODE
```
id              UUID v7 — stable forever
type            document | section | heading | paragraph | sentence |
                list_item | blockquote | code_block | table | figure
text            (sentences only — plain text slice from parent para)
parent_id       UUID of parent (null for document root)
children        ordered array of child UUIDs
depth           0=document, 1=section, 2=paragraph, 3=sentence
structural_hash SHA-256 of normalized plain text
source_doc_id   UUID of containing document
source_format   markdown | plaintext | transcript | ...
text_start      char offset in original source
text_end        char offset in original source
version         integer, incremented on change
created_at
updated_at
```

### DOCUMENT
```
id              UUID v7
title
format
source_path     file path or URI
source_hash     SHA-256 of full source text
root_node_id    UUID of document-level node
ingested_at
version
```

### ANNOTATION (W3C Web Annotation)
```
id              UUID v7
type            highlight | comment | tag | link | provenance | ai-note | ...
target_nodes    array of node UUIDs
target_ranges   [{node_id, start_offset, end_offset}] for sub-sentence targeting
selectors:
  position:     {start: int, end: int}         ← fast path
  quote:        {exact, prefix, suffix}         ← recovery path
confidence      0.0–1.0
status          active | active_partial | orphan | deleted
payload         type-specific data
created_at
updated_at
```

### TRANSCLUSION
```
id              UUID v7
predicate       transcludes | cites | derives-from | responds-to |
                supports | contradicts | supersedes | exemplifies
source_node_id  UUID — the referenced node
source_doc_id
target_node_id  UUID — the slot in the including document
target_doc_id
version_at_include
status          live | stale | orphaned
```

### PROVENANCE (on any node)
```
source_uri      document://doc-uuid#node-uuid
char_span       {start, end} in original source
confidence      extraction confidence
extractor       parser name / agent name
derived_from    UUID of the node this was transformed from
derived_at
```

### ACTIVITY (append-only)
```
id              UUID v7
type            parse | edit | annotate | transclude | chunk | embed | ...
input_ids       node UUIDs that were inputs
output_ids      node UUIDs produced
agent           who/what performed this
started_at
ended_at
```

---

## Re-Ingestion Strategy

Re-ingestion is a diff operation against the stored tree, not a fresh parse.

```
LEVEL 1 — EXACT MATCH
  structural_hash matches + position within ±1 sibling → keep UUID

LEVEL 2 — FUZZY MATCH
  text similarity > threshold + position within ±2 siblings
  → keep UUID, update text + hash + version

LEVEL 3 — NEW NODE
  no match → mint new UUID v7

LEVEL 4 — DELETED NODE
  existing UUID not matched in new tree
  → mark deleted, keep record
  → annotations move to orphan state (never silently dropped)
```

---

## Annotation Re-Anchoring (4-Strategy Cascade)

Runs on every re-ingestion for each annotation:

```
1. UUID exact match, same hash     → confidence 1.0, status: active
2. UUID match, hash changed        → confidence 0.8, status: active_partial
3. TextQuoteSelector match         → confidence varies, status: active_partial
4. No match                        → confidence 0.0, status: orphan
```

Dual selectors reconciled at **write time** (position + quote describe same span).
At **read time**: position selector used first (fast), quote selector as fallback.
Fuzzy matching only on the write side — render side uses verbatim only.

---

## Agent Interface

### §N Sentence Markers (cursor mechanism)
When projecting a document for an agent, the runtime injects sentence markers:

```
§1 The core problem with context compression is that...
§2 Most retrieval systems treat documents as flat bags of chunks.
§3 A structural addressing layer would change this.
```

Markers are session-local and ephemeral — they do not persist. They are the agent's cursor into the document, exactly as line numbers serve the code agent.

### Three Precision Levels
```
Whole sentence:       { sentence: 14 }
Sub-sentence:         { sentence: 14, quote: "quantum entanglement" }
Sentence range:       { sentences: [14, 15, 16] }
```

The agent never touches UUIDs, positions, char offsets, or prefix/suffix context.

### Resolution Chain
```
§14 → look up session-local §→UUID map
    → get node UUID
    → get node text (read content file, slice by char_start/char_end)
    → if quote: search for "quantum entanglement" within that sentence only
      (bounded search → near-zero ambiguity, no prefix/suffix needed)
    → build TextPositionSelector + TextQuoteSelector
    → store W3C annotation JSON-LD in SQLite
```

---

## Query Contract
```
getNode(uuid)              → Node
getDocument(uuid)          → Document
getChildren(uuid)          → Node[]
getAncestors(uuid)         → Node[]
getSiblings(uuid)          → Node[]
search(text)               → Node[]    (FTS5)
getAnnotations(uuid)       → Annotation[]
getTransclusions(uuid)     → Transclusion[]
getProvenance(uuid)        → ProvenanceChain
resolveTransclusion(uuid)  → Node
```

## Projection Contract
```
projectMarkdown(uuid)      → clean Markdown (UUIDs invisible)
projectPlaintext(uuid)     → plain text
projectHTML(uuid)          → HTML with data-node-id attributes
projectAgentBrief(uuid)    → §N-marked text + epistemic state (NOT YET BUILT)
```

---

## The Full Shape
```
                ┌─────────────┐
                │  RAW TEXT   │   write flow — human, uninterrupted
                └──────┬──────┘
                       │ ingest trigger
                       ▼
        ┌──────────────────────────────────┐
        │         INGESTION PIPELINE       │
        │  normalize → pandoc → segment    │
        │  → assign UUIDs → store → index  │
        └──────────────┬───────────────────┘
                       │
          ┌────────────┼────────────┐
          ▼            ▼            ▼
    OBJECT STORE   ANNOTATION   TRANSCLUSION
    (node tree)    OVERLAY      LAYER
          │            │            │
          └────────────┼────────────┘
                       │
                  QUERY LAYER
                       │
          ┌────────────┼────────────┐
          ▼            ▼            ▼
    PROJECTION    DISCOVERY    AGENT BRIEF
    (read flow)   (not built)  (not built)
                       │
                ┌─────────────┐
                │   AGENT /   │   primary consumer
                │   HUMAN     │
                └─────────────┘
```
