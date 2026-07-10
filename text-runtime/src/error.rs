// ── TextRuntimeError — All failure modes of the Text Runtime ────────────────
// Uses thiserror 2.0 derive for Display + Error + From implementations.

use std::path::PathBuf;
use thiserror::Error;

/// All failure modes of the Text Runtime.
///
/// Variants are grouped by subsystem: runtime lifecycle, configuration,
/// UUID/document/node lookup, ingestion/parsing, annotation, transclusion,
/// storage, search, pandoc manager, daemon, and internal errors.
#[derive(Error, Debug)]
pub enum TextRuntimeError {
    // ── Runtime lifecycle ──────────────────────────────────────────────
    #[error("runtime has been closed — cannot perform operations")]
    RuntimeClosed,

    #[error("runtime directory '{0}' is not accessible: {1}")]
    RuntimeAccessError(PathBuf, String),

    // ── Configuration ──────────────────────────────────────────────────
    #[error("configuration error: {0}")]
    ConfigError(String),

    #[error("unsupported format: '{0}' (valid formats: {1})")]
    UnsupportedFormat(String, String),

    // ── UUID / Document / Node lookup ─────────────────────────────────
    #[error("invalid UUID: '{0}'")]
    InvalidUuid(String),

    #[error("document not found: '{0}'")]
    DocumentNotFound(String),

    #[error("node not found: '{0}'")]
    NodeNotFound(String),

    // ── Ingestion / Parsing ────────────────────────────────────────────
    #[error("parse error for format '{format}': {message}")]
    ParseError { format: String, message: String },

    #[error("pandoc-server returned an error for document '{doc}': {message}")]
    PandocConversionError { doc: String, message: String },

    #[error("pandoc-server timeout after {0}ms")]
    PandocTimeout(u64),

    #[error("no content extracted — document appears to be empty")]
    EmptyDocument,

    // ── Annotation ────────────────────────────────────────────────────
    #[error("annotation resolution failed: {0}")]
    AnnotationResolutionError(String),

    #[error(
        "selector reconciliation failed: position ({pos_start},{pos_end}) ≠ quote resolved to ({quote_start},{quote_end})"
    )]
    SelectorReconciliationFailed {
        pos_start: usize,
        pos_end: usize,
        quote_start: usize,
        quote_end: usize,
    },

    #[error("empty span — cannot annotate zero-length text")]
    EmptyAnnotationSpan,

    // ── Transclusion ──────────────────────────────────────────────────
    #[error("invalid transclusion predicate: '{0}' (valid: {1})")]
    InvalidPredicate(String, String),

    #[error("transclusion edge not found: '{0}'")]
    TransclusionNotFound(String),

    #[error("circular transclusion detected: {0}")]
    CircularTransclusion(String),

    // ── Storage ───────────────────────────────────────────────────────
    #[error("database error: {0}")]
    DatabaseError(#[from] rusqlite::Error),

    #[error("I/O error at '{path}': {source}")]
    IoError {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("content file not found: '{0}'")]
    ContentFileNotFound(String),

    #[error("content file corrupt at '{path}': {message}")]
    ContentFileCorrupt { path: PathBuf, message: String },

    #[error("serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    // ── Search ────────────────────────────────────────────────────────
    #[error("FTS5 query error: {0}")]
    Fts5QueryError(String),

    // ── Pandoc Manager ────────────────────────────────────────────────
    #[error("pandoc-server not running (tried port {0})")]
    PandocServerNotRunning(u16),

    #[error("pandoc-server crashed after {restarts} restarts")]
    PandocServerCrashed { restarts: u32 },

    #[error("pandoc-server health check failed after {0}ms")]
    PandocHealthCheckTimeout(u64),

    // ── Daemon ────────────────────────────────────────────────────────
    #[error("file watcher error: {0}")]
    FileWatcherError(String),

    #[error("daemon is already running")]
    DaemonAlreadyRunning,

    #[error("daemon lock error: {0}")]
    DaemonLockError(String),

    #[error("socket error: {0}")]
    SocketError(String),

    // ── Internal ──────────────────────────────────────────────────────
    #[error("internal error: {0}")]
    InternalError(String),
}

impl TextRuntimeError {
    /// Convenience constructor: wrap a `std::io::Error` with path context.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        TextRuntimeError::IoError {
            path: path.into(),
            source,
        }
    }
}
