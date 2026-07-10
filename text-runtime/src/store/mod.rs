// ── Store Module ────────────────────────────────────────────────────────────
//
// The store module owns both the SQLite database store and the content
// file store. Together they form the two-store architecture:
// - SQLite: structure, identity, relations (NO text content)
// - ContentStore: Pandoc AST JSON files, one per block node

pub mod content;
pub mod db;
pub mod queries;
pub mod types;

pub use content::ContentStore;
pub use db::{sanitize_fts_query, DbStore, SearchResultRow};
pub use types::*;

use std::path::Path;

use crate::cfg::RuntimeConfig;
use crate::error::TextRuntimeError;

/// Top-level Store that owns both DbStore and ContentStore.
///
/// This is the main entry point for storage operations. It holds the
/// database connection, the content file store, and the runtime
/// configuration.
pub struct Store {
    /// SQLite database for structural data, annotations, transclusions,
    /// activities, and FTS5 full-text search.
    pub db: DbStore,

    /// Filesystem content store for Pandoc AST JSON files.
    pub content: ContentStore,

    /// Runtime configuration (pandoc settings, thresholds, locale).
    pub config: RuntimeConfig,
}

impl Store {
    /// Open a new Store at the given runtime directory.
    ///
    /// Creates (or opens) the SQLite database at `{runtime_dir}/db.sqlite`,
    /// the content store at `{runtime_dir}/content/`, and loads the config
    /// from `{runtime_dir}/config.json`.
    pub fn open(runtime_dir: &Path) -> Result<Self, TextRuntimeError> {
        let db_path = runtime_dir.join("db.sqlite");
        let content_path = runtime_dir.join("content");
        let tmp_path = runtime_dir.join("tmp");

        // Create tmp directory for atomic writes
        std::fs::create_dir_all(&tmp_path).map_err(|e| TextRuntimeError::io(&tmp_path, e))?;

        let db = DbStore::open(&db_path)?;
        let content = ContentStore::new(content_path)?;
        let config = RuntimeConfig::load_or_create(runtime_dir)?;

        Ok(Self {
            db,
            content,
            config,
        })
    }

    /// Close the store, consuming it.
    ///
    /// Closes the SQLite database connection. Content files are on disk
    /// and don't need explicit closing.
    pub fn close(self) -> Result<(), TextRuntimeError> {
        self.db.close()
    }
}
