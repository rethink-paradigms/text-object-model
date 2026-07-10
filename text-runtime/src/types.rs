// ── Shared Types ──────────────────────────────────────────────────────────────
// Type aliases and core structs used across all modules.

/// Re-export uuid::Uuid as the canonical Uuid type throughout the runtime.
pub type Uuid = uuid::Uuid;

/// Position within flat plain text with path through inline tree.
///
/// Used during ingest (forward mapping) and projection (reverse mapping)
/// to translate between flat text character offsets and positions in the
/// Pandoc inline array.
#[derive(Debug, Clone)]
pub struct TextPosition {
    /// Start byte offset in the flat plain text.
    pub flat_start: usize,
    /// End byte offset in the flat plain text.
    pub flat_end: usize,
    /// Path through nested inline elements: one entry per depth level,
    /// each holding the child index at that level and offset info.
    pub inline_stack: Vec<TextOffsetInInline>,
    /// If true, this range MUST NOT be split. Sentence boundaries that
    /// fall inside are pushed to the nearest safe boundary.
    pub is_atomic: bool,
}

/// Position within a single inline element at a given depth in the inline tree.
#[derive(Debug, Clone)]
pub struct TextOffsetInInline {
    /// Index into the parent container's `Vec<Inline>` at this depth.
    pub inline_index: usize,
    /// Byte offset within that inline element's own flat text contribution.
    pub offset_within_inline: usize,
    /// Inline variant name for debugging: "Str", "Emph", "Strong", etc.
    pub inline_kind: &'static str,
}

/// Stack of child indices for navigating nested inline formatting.
///
/// Each `usize` is the child index at one depth level. Used for
/// lightweight traversal of the inline tree without carrying full
/// position information.
#[derive(Debug, Clone)]
pub struct InlineStack {
    /// Child indices at each depth level, outermost first.
    pub path: Vec<usize>,
}

/// A sentence span within a parent paragraph's flat text.
///
/// Stored in SQLite as `char_start` and `char_end` columns on sentence
/// nodes. Both values are UTF-8 **byte** offsets into the parent
/// paragraph's `plain_text`.
#[derive(Debug, Clone)]
pub struct SentenceSpan {
    /// Byte offset in parent's plain_text where the sentence begins.
    pub char_start: usize,
    /// Byte offset in parent's plain_text where the sentence ends.
    pub char_end: usize,
}

/// §N → Uuid mapping, session-local and ephemeral.
///
/// Built during `read()` (projection with markers) and consumed by
/// `annotate()` to resolve sentence numbers back to node UUIDs.
/// Not persisted — discarded when the session ends.
pub type MarkerMap = std::collections::HashMap<u32, Uuid>;

/// Session identifier for marker map resolution.
///
/// Each `read()` call with `markers: true` creates a new session ID.
/// The agent passes this ID to subsequent `annotate()` calls so the
/// runtime can resolve `§N` → `Uuid`.
pub type SessionId = Uuid;

/// Structural node produced by the segmenter before SQLite insertion.
///
/// Built by walking the Pandoc AST. Contains all information needed
/// for both the SQLite `nodes` table and the content file store.
/// The `uuid` field is `None` until UUID assignment (during ingest).
#[derive(Debug, Clone)]
pub struct StructuralNode {
    /// UUID — `None` until the UUID allocator assigns one.
    pub uuid: Option<Uuid>,
    /// Structural node type: Document, Section, Paragraph, Heading, etc.
    pub node_type: NodeType,
    /// Parent node UUID (None for document root).
    pub parent_uuid: Option<Uuid>,
    /// Gap-based float ordering within siblings (1000, 2000, ...).
    pub position: f64,
    /// Extracted plain text (from Pandoc AST inline walk).
    pub plain_text: String,
    /// SHA-256 hex string (64 chars) of normalized plain_text.
    pub structural_hash: String,
    /// 1 = content file exists, 0 = derived (sentence/container).
    pub has_content: bool,
    /// Sentence only: byte offset into parent paragraph plain_text.
    pub char_start: Option<usize>,
    /// Sentence only: byte offset into parent paragraph plain_text.
    pub char_end: Option<usize>,
    /// Heading level 1-6; None for non-heading nodes.
    pub heading_level: Option<i32>,
    /// Dot-separated heading numbers: "1.2.3".
    pub section_path: Option<String>,
    /// Incremented on fuzzy match re-ingestion, defaults to 1.
    pub version: i32,
    /// Child structural nodes (built during tree walk, consumed by SQLite writer).
    pub children: Vec<StructuralNode>,
    /// Pandoc AST fragment for the content file (block nodes only).
    pub pandoc_ast_json: Option<serde_json::Value>,
}

/// Structural node type — maps directly to Pandoc AST block types
/// plus the runtime-added `Sentence` type (produced by segmentation).
#[derive(Debug, Clone, PartialEq)]
pub enum NodeType {
    Document,
    Section,
    Paragraph,
    Heading,
    Sentence,
    CodeBlock,
    ListItem,
    Table,
    BlockQuote,
    ThematicBreak,
}

impl NodeType {
    /// Return the string representation used in SQLite's `node_type` column.
    pub fn as_str(&self) -> &'static str {
        match self {
            NodeType::Document => "document",
            NodeType::Section => "section",
            NodeType::Paragraph => "paragraph",
            NodeType::Heading => "heading",
            NodeType::Sentence => "sentence",
            NodeType::CodeBlock => "code_block",
            NodeType::ListItem => "list_item",
            NodeType::Table => "table",
            NodeType::BlockQuote => "blockquote",
            NodeType::ThematicBreak => "thematic_break",
        }
    }

    /// Parse a string from SQLite's `node_type` column back to NodeType.
    /// Returns `None` for unrecognized values.
    ///
    /// Named `from_sql` (not `from_str`) to avoid shadowing
    /// `std::str::FromStr::from_str`.
    pub fn from_sql(s: &str) -> Option<Self> {
        match s {
            "document" => Some(NodeType::Document),
            "section" => Some(NodeType::Section),
            "paragraph" => Some(NodeType::Paragraph),
            "heading" => Some(NodeType::Heading),
            "sentence" => Some(NodeType::Sentence),
            "code_block" => Some(NodeType::CodeBlock),
            "list_item" => Some(NodeType::ListItem),
            "table" => Some(NodeType::Table),
            "blockquote" => Some(NodeType::BlockQuote),
            "thematic_break" => Some(NodeType::ThematicBreak),
            _ => None,
        }
    }
}
