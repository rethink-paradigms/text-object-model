// ── SQLite Row Types ────────────────────────────────────────────────────────
//
// Each struct maps to a row in a SQLite table. All have:
//   - `id` (INTEGER PK, internal, used for FTS5 content_rowid)
//   - `uuid` (TEXT, external identity, stable forever)
// plus table-specific columns per schema.md.

/// Row from the `documents` table.
#[derive(Debug, Clone)]
pub struct DocumentRow {
    pub id: i64,
    pub uuid: String,
    pub title: String,
    pub import_format: String,
    pub import_path: Option<String>,
    pub import_hash: Option<String>,
    pub root_node_uuid: Option<String>,
    pub version: i32,
    pub ingested_at: String,
    pub language: String,
}

/// Row from the `nodes` table.
///
/// Stores structural identity, position in the tree, content addressing,
/// sentence offsets, heading hierarchy, and versioning.
#[derive(Debug, Clone)]
pub struct NodeRow {
    pub id: i64,
    pub uuid: String,
    pub doc_id: String,
    pub node_type: String,
    pub parent_uuid: Option<String>,
    pub position: f64,
    pub has_content: bool,
    pub content_path: Option<String>,
    pub plain_text: String,
    pub structural_hash: String,
    pub char_start: Option<i64>,
    pub char_end: Option<i64>,
    pub heading_level: Option<i32>,
    pub section_path: Option<String>,
    pub version: i32,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Row from the `annotations` table.
///
/// Stores the full W3C JSON-LD annotation blob with denormalized
/// columns for fast SQL queries (targeting, motivation, status).
#[derive(Debug, Clone)]
pub struct AnnotationRow {
    pub id: i64,
    pub uuid: String,
    /// Full W3C Web Annotation JSON-LD blob.
    pub annotation: String,
    /// Denormalized from annotation JSON for indexing.
    pub target_uuid: String,
    pub target_doc_id: String,
    pub motivation: String,
    pub status: String,
    pub creator: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Row from the `transclusions` table.
///
/// A typed directed edge between nodes across documents.
#[derive(Debug, Clone)]
pub struct TransclusionRow {
    pub id: i64,
    pub uuid: String,
    pub predicate: String,
    pub source_node_uuid: String,
    pub source_doc_uuid: String,
    pub target_node_uuid: String,
    pub target_doc_uuid: String,
    pub version_at_include: i32,
    pub status: String,
    pub created_at: String,
}

/// Row from the `activities` table.
///
/// Append-only — activities are events, never updated.
/// `input_ids` and `output_ids` are JSON arrays of UUIDs.
/// `config` is a JSON object of parameters.
#[derive(Debug, Clone)]
pub struct ActivityRow {
    pub id: i64,
    pub uuid: String,
    pub activity_type: String,
    /// JSON array of consumed UUIDs.
    pub input_ids: Option<String>,
    /// JSON array of produced UUIDs.
    pub output_ids: Option<String>,
    pub agent: Option<String>,
    /// JSON object of parameters.
    pub config: Option<String>,
    pub started_at: String,
    pub ended_at: Option<String>,
}
