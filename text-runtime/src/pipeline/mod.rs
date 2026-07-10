// ── Pipeline Module ─────────────────────────────────────────────────────────
//
// The ingestion pipeline: format detection → parsing → inline mapping →
// structural segmentation → UUID assignment → content writing →
// SQLite writing → FTS indexing → activity logging → orchestration.

pub mod activity_logger;
pub mod content_writer;
pub mod format;
pub mod fts_indexer;
pub mod ingest;
pub mod inline_mapping;
pub mod parser;
pub mod segmenter;
pub mod sqlite_writer;
pub mod uuid_assigner;

pub use ingest::{run_pipeline, IngestResult};
