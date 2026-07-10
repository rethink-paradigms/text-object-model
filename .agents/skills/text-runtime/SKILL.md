---
name: text-runtime
description: Learn to use the text-runtime tool for document ingestion, annotation, and searching.
---

# Text Runtime Skill

The `text-runtime` is a powerful local-first text management engine. It allows you to search across documents, read documents with exact sentence boundaries, and anchor annotations reliably.

When the user asks you to read or research within a document managed by this runtime, or to search across the library, use `run_command` with the `text-runtime` CLI.

## Usage

You can use the helper script `text-runtime.sh` or run the commands directly using `cargo run`. 

**1. Ingest a document**
`cargo run -- ingest path/to/doc.md`
This returns the `doc_id`.

**2. Read a document (with sentence markers)**
`cargo run -- read <doc_id> --markers`
This returns the document text with `§N` markers injected at sentence boundaries, making it easy to identify the exact target for an annotation.

**3. Annotate a sentence**
`cargo run -- annotate <doc_id> --sentence <N> --body "Your comment here"`
This attaches an annotation to the specified sentence number.

**4. Search**
`cargo run -- search "<query>"`
This runs a full-text search across all ingested documents.
