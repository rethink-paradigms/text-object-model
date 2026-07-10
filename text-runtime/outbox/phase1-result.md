## Result: Phase 1 Foundation — text-runtime crate compiles and passes all tests

status: done

## Changes made

### Cargo.toml
- Removed `[[bin]]` section (referenced non-existent `src/main.rs`)
- Removed `[[bench]]` section (referenced non-existent `benches/pipeline.rs`)
- Changed `rusqlite` features from `["bundled", "fts5", "column_decltype"]` to `["bundled"]` — `fts5` and `column_decltype` features don't exist in rusqlite 0.31 (FTS5 is included in bundled SQLite builds)

### src/lib.rs
- Removed `pub mod pandoc_mgr;` — Phase 2+
- Removed `pub mod pipeline;` — Phase 2+ (several pipeline submodules don't exist yet)
- Removed `pub mod reingest;` — file doesn't exist yet

Now only exposes Phase 1 modules: `cfg`, `error`, `types`, `uuid7`, `store`

### src/store/content.rs
- Removed unused `Path` import (line 7: `use std::path::PathBuf;` only)

### src/store/db.rs
- Fixed lifetime issue in `detect_stale_edges()` (lines 831–848): `MappedRows` borrows from `stmt`, but `stmt` was dropped before the collection completed. Restructured if-else branches to assign the collected `Vec` to a `result` variable before returning from each branch.

## Existing files (confirmed working, no changes needed)

1. **src/types.rs** — All shared types: `TextPosition`, `TextOffsetInInline`, `SentenceSpan`, `MarkerMap`, `SessionId`, `StructuralNode`, `NodeType` enum
2. **src/error.rs** — `TextRuntimeError` enum with all 30+ variants from api-surface.md, `#[from]` derives, `io()` helper
3. **src/uuid7.rs** — `uuid7()` function, `UuidAllocator` struct with `new()`, `allocate()`, `allocate_dedup()`, `contains()`, 4 unit tests
4. **src/cfg.rs** — `RuntimeConfig` with serde defaults, `Default` impl, `load_or_create()`, `save()`, 2 unit tests
5. **src/store/types.rs** — All 5 row types: `DocumentRow`, `NodeRow`, `AnnotationRow`, `TransclusionRow`, `ActivityRow`
6. **src/store/content.rs** — `ContentStore` with fanout paths, `get`, `put` (atomic), `delete`, `exists`, 3 unit tests
7. **src/store/db.rs** — `DbStore` with full schema (5 tables, FTS5, 3 triggers, all pragmas), 15+ CRUD methods, `sanitize_fts_query`, transaction support, 4 unit tests
8. **src/store/mod.rs** — Re-exports and top-level `Store` struct owning both DbStore and ContentStore

## Validation

```
$ cargo build
   Compiling text-runtime v0.1.0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.04s

$ cargo test
running 13 tests
test cfg::tests::test_default_config ... ok
test store::db::tests::test_sanitize_fts_query ... ok
test cfg::tests::test_save_load_roundtrip ... ok
test store::db::tests::test_insert_get_node ... ok
test uuid7::tests::test_uuid7_format ... ok
test uuid7::tests::test_dedup_with_candidate ... ok
test uuid7::tests::test_uuid7_monotonic ... ok
test store::db::tests::test_fts5_search ... ok
test store::db::tests::test_schema_init ... ok
test uuid7::tests::test_dedup_10k ... ok
test store::content::tests::test_fanout_path ... ok
test store::content::tests::test_put_get_roundtrip ... ok
test store::content::tests::test_atomic_write_no_partial_reads ... ok

test result: ok. 13 passed; 0 failed; 0 ignored
```

Zero compiler warnings. Zero test failures.

## Notes

### Minor spec deviations (intentional improvements)
- `Uuid` type is `uuid::Uuid` (not `String`) — more type-safe, works directly with `uuid` crate
- `SessionId` is a type alias (`pub type SessionId = Uuid`) rather than newtype — equivalent semantics
- `StructuralNode.uuid` is `Option<Uuid>` to support pre-assignment phase during ingest
- Content store methods accept `&Uuid` (not `&str`) for type safety
- `DbStore` constructor is named `open()` (not `new()`) consistent with std library conventions
- Some DB method names differ slightly from task spec (e.g., `get_document_by_path` vs `get_document_by_source`, `get_children` vs `get_nodes_by_parent`) — same functionality

### `lint_file` tool unavailable
The Python linter failed with `ModuleNotFoundError: No module named 'grep_ast'`. Rust's built-in compiler (`cargo build`, `cargo test`) provided full validation — zero warnings, zero errors.

### Next steps (Phase 2+)
Phase 2 would add: `pandoc_mgr.rs`, `reingest.rs`, `pipeline/` submodules (content_writer, sqlite_writer, fts_indexer, activity_logger, ingest), `src/main.rs`
