# Text Runtime

Local-first text runtime — ingest, structure, annotate, project.

A Rust library and CLI tool for structural text processing with UUID-stable identity, W3C Web Annotations, and content-addressed persistence.

## Architecture

```
┌──────────────────────────────────────────────────┐
│                   Runtime                        │
│  (Store + PandocManager + MarkerMap)             │
├──────────────────────────────────────────────────┤
│  ┌──────────┐  ┌──────────┐  ┌───────────────┐  │
│  │  SQLite  │  │ Content  │  │  Pandoc       │  │
│  │  (DbStore)│  │  Store   │  │  Server       │  │
│  └──────────┘  └──────────┘  └───────────────┘  │
├──────────────────────────────────────────────────┤
│  Pipeline: format → parse → segment → assign    │
│            → write → index → log                │
├──────────────────────────────────────────────────┤
│  Projection: reverse — DB → AST → render        │
├──────────────────────────────────────────────────┤
│  Annotations: W3C JSON-LD + dual selectors      │
│  Transclusions: typed directed edges             │
└──────────────────────────────────────────────────┘
```

## Features

- **Ingest** — Parse Markdown, LaTeX, HTML, DOCX, and more via pandoc
- **Structure** — Sentence segmentation with ICU, atomic boundary push
- **Identity** — UUID v7 per node, content-addressed hashing, re-ingestion merge
- **Project** — Read documents in any output format with §N markers
- **Annotate** — W3C Web Annotations with dual position+quote selectors
- **Search** — FTS5 full-text search with BM25 ranking
- **Transclude** — Typed directed edges between nodes

## Quick Start

```bash
# Ingest a document
cargo run -- ingest path/to/document.md

# Read a document
cargo run -- read <doc-uuid> --format markdown --markers

# Search
cargo run -- search "search query"

# Run the daemon (config: ~/.config/text-runtime/config.toml; see docs/RUNBOOK.md)
cargo run -- daemon --config ~/.config/text-runtime/config.toml
```

## Library Usage

```rust
use text_runtime::runtime::Runtime;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut runtime = Runtime::open(".textruntime".as_ref()).await?;

    // Ingest a file
    let doc_id = runtime.ingest_file("doc.md".as_ref()).await?;

    // Read with markers
    let projection = runtime.read(&doc_id, "markdown", true)?;
    println!("{}", projection.text);

    // Search
    let hits = runtime.search("hello", None)?;

    runtime.close().await?;
    Ok(())
}
```

## Storage

Two-store architecture:

1. **SQLite** — Structural metadata, annotations, transclusions, FTS5
2. **Content files** — Pandoc AST JSON, one per block node, 256-bucket fanout

## Requirements

- Rust 1.82+
- pandoc-server (optional, for full ingestion/projection)

## License

MIT OR Apache-2.0
