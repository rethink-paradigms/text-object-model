// ── Full Pipeline Orchestration ─────────────────────────────────────────────
//
// Runs the complete ingestion pipeline:
//   1. Detect format + normalize text
//   2. Parse via pandoc-server → Pandoc AST
//   3. Segment into structural nodes
//   4. Compute structural_hash + assign UUIDs
//   5. Write content files for has_content nodes
//   6. INSERT/UPDATE in SQLite (single transaction)
//   7. FTS sync (automatic via triggers)
//   8. Log activity

use crate::error::TextRuntimeError;
use crate::pandoc_mgr::PandocManager;
use crate::pipeline::activity_logger;
use crate::pipeline::content_writer;
use crate::pipeline::format;
use crate::pipeline::fts_indexer;
use crate::pipeline::parser;
use crate::pipeline::segmenter;
use crate::pipeline::sqlite_writer;
use crate::pipeline::uuid_assigner;
use crate::reingest;
use crate::store::types::DocumentRow;
use crate::store::Store;
use crate::types::{NodeType, StructuralNode};
use crate::uuid7::uuid7;
use crate::uuid7::UuidAllocator;

/// Public result type returned by ingest operations.
#[derive(Debug, Clone)]
pub struct IngestResult {
    pub document_uuid: String,
    pub node_count: usize,
    pub sentence_count: usize,
    pub new_nodes: usize,
    pub updated_nodes: usize,
    pub deleted_nodes: usize,
    pub activity_uuid: String,
}

/// Run the complete ingestion pipeline.
///
/// # Arguments
///
/// * `text` - Raw text to ingest
/// * `format` - Explicit Pandoc format ("markdown", "latex", etc.). If None,
///   auto-detected from source_path extension.
/// * `title` - Document title
/// * `source_path` - Optional source file path (for provenance)
/// * `store` - The Store (SQLite + content files)
/// * `pandoc` - PandocManager for AST parsing
/// * `merge` - If true, perform re-ingestion for existing document by source_path
///
/// # Returns
///
/// IngestResult with document UUID, node counts, and activity UUID.
pub enum IngestInput<'a> {
    Text(&'a str),
    BinaryFile(&'a std::path::Path),
}

pub async fn run_pipeline(
    input: IngestInput<'_>,
    format: Option<&str>,
    title: &str,
    source_path: Option<&str>,
    store: &mut Store,
    pandoc: &PandocManager,
    merge: bool,
) -> Result<IngestResult, TextRuntimeError> {
    // ── Stage 1: Format detection + normalization ────────────────────────
    let format_str = format::detect_format(source_path, format)?;

    // ── Stage 2: Parse to Pandoc AST ─────────────────────────────────────
    let (pandoc_ast, original_hash) = match input {
        IngestInput::Text(t) => {
            let normalized = format::prepare_input(t, source_path, format)?;
            let ast = parser::parse_to_ast(pandoc, &normalized.text, &normalized.format).await?;
            (ast, normalized.original_hash)
        }
        IngestInput::BinaryFile(path) => {
            use sha2::{Digest, Sha256};
            let bytes = std::fs::read(path).map_err(|e| {
                TextRuntimeError::InternalError(format!("Failed to read binary file: {e}"))
            })?;
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            let hash = format!("{:x}", hasher.finalize());
            let ast = parser::parse_file_to_ast(pandoc, path, &format_str).await?;
            (ast, hash)
        }
    };

    if pandoc_ast.blocks.is_empty() {
        return Err(TextRuntimeError::EmptyDocument);
    }

    // ── Stage 3: Segment into structural nodes ───────────────────────────
    let mut root = segmenter::segment(&pandoc_ast)?;

    // ── Stage 4: UUID assignment ────────────────────────────────────────
    let mut allocator = UuidAllocator::new();

    // Generate document UUID
    let doc_uuid = if merge {
        // Re-ingestion: look up existing document by source_path
        if let Some(path) = source_path {
            if let Some(existing_doc) = store.db.get_document_by_path(path)? {
                existing_doc.uuid
            } else {
                uuid7().to_string()
            }
        } else {
            uuid7().to_string()
        }
    } else {
        uuid7().to_string()
    };

    // Determine if this is a re-ingestion
    let mut deleted_count: usize = 0;
    let mut diff_ran = false;
    if merge {
        if let Some(existing_doc) = source_path
            .and_then(|p| store.db.get_document_by_path(p).ok())
            .flatten()
        {
            println!("DEBUG: Found existing doc: {}", existing_doc.uuid);
            // Get existing nodes for diffing
            let existing_nodes = store.db.get_nodes_by_doc(&existing_doc.uuid)?;
            println!("DEBUG: Found {} existing nodes", existing_nodes.len());

            // Perform diff and merge
            let (kept, updated, deleted) = reingest::diff_and_merge(
                &mut root,
                &existing_doc.uuid,
                &existing_nodes,
                &mut store.db,
                &mut allocator,
                store.config.fuzzy_match_threshold,
            )?;
            println!(
                "DEBUG: diff_and_merge result: kept={}, updated={}, deleted={}",
                kept, updated, deleted
            );

            deleted_count = deleted;
            diff_ran = true;
        }
    }

    if !diff_ran {
        // No existing document found or not merging — treat as new ingest
        uuid_assigner::assign_uuids(&mut root, &doc_uuid, &store.db, &mut allocator)?;
    }

    // Set root node UUID on the document node
    root.uuid = root.uuid.or_else(|| Some(allocator.allocate()));
    root.plain_text = collect_plain_text(&root);
    root.structural_hash = uuid_assigner::compute_structural_hash(&root.plain_text);

    // ── Stage 5: Write content files ────────────────────────────────────
    content_writer::write_content_files(&root, &store.content)?;

    // ── Stage 6: SQLite insert/update ───────────────────────────────────
    let now = chrono::Utc::now().to_rfc3339();

    let root_node_uuid = root.uuid.map(|u| u.to_string());

    let doc = DocumentRow {
        id: 0,
        uuid: doc_uuid.clone(),
        title: title.to_string(),
        import_format: format_str.clone(),
        import_path: source_path.map(|s| s.to_string()),
        import_hash: Some(original_hash.clone()),
        root_node_uuid: root_node_uuid.clone(),
        version: 1,
        ingested_at: now.clone(),
        language: store.config.locale.clone(),
    };

    let (new_nodes, updated_nodes) = sqlite_writer::write_to_sqlite(&mut store.db, &doc, &root)?;

    // ── Stage 7: FTS sync ────────────────────────────────────────────────
    fts_indexer::ensure_fts_synced(&store.db)?;

    // ── Stage 8: Log activity ────────────────────────────────────────────
    // Collect all output UUIDs
    let mut output_ids: Vec<String> = Vec::new();
    output_ids.push(doc_uuid.clone());
    collect_node_uuids(&root, &mut output_ids);

    let config_json = serde_json::json!({
        "format": format_str,
        "merge": merge,
        "fuzzy_threshold": store.config.fuzzy_match_threshold,
    })
    .to_string();

    let input_ids: Vec<String> = if let Some(path) = source_path {
        if let Ok(Some(existing)) = store.db.get_document_by_path(path) {
            vec![existing.uuid]
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    let activity_uuid = activity_logger::log_ingest_activity(
        &mut store.db,
        if merge { "reingest" } else { "ingest" },
        &input_ids,
        &output_ids,
        "text-runtime",
        &config_json,
    )?;

    // ── Count statistics ────────────────────────────────────────────────
    let node_count = count_nodes(&root);
    let sentence_count = count_sentences(&root);

    Ok(IngestResult {
        document_uuid: doc_uuid,
        node_count,
        sentence_count,
        new_nodes: new_nodes.len(),
        updated_nodes: updated_nodes.len(),
        deleted_nodes: deleted_count,
        activity_uuid,
    })
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Collect plain text from all children recursively.
fn collect_plain_text(node: &StructuralNode) -> String {
    if node.node_type == NodeType::Sentence {
        return node.plain_text.clone();
    }

    let mut parts = Vec::new();
    for child in &node.children {
        let child_text = collect_plain_text(child);
        if !child_text.is_empty() {
            parts.push(child_text);
        }
    }

    if parts.is_empty() {
        node.plain_text.clone()
    } else {
        parts.join("\n")
    }
}

/// Collect all node UUIDs recursively.
fn collect_node_uuids(node: &StructuralNode, uuids: &mut Vec<String>) {
    if let Some(uuid) = &node.uuid {
        uuids.push(uuid.to_string());
    }
    for child in &node.children {
        collect_node_uuids(child, uuids);
    }
}

/// Count total nodes in tree.
fn count_nodes(node: &StructuralNode) -> usize {
    1 + node.children.iter().map(count_nodes).sum::<usize>()
}

/// Count sentence nodes in tree.
fn count_sentences(node: &StructuralNode) -> usize {
    let self_count = if node.node_type == NodeType::Sentence {
        1
    } else {
        0
    };
    self_count + node.children.iter().map(count_sentences).sum::<usize>()
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_nodes() {
        let root = StructuralNode {
            version: 1,
            uuid: None,
            node_type: NodeType::Document,
            parent_uuid: None,
            position: 0.0,
            plain_text: String::new(),
            structural_hash: String::new(),
            has_content: false,
            char_start: None,
            char_end: None,
            heading_level: None,
            section_path: None,
            children: vec![StructuralNode {
                version: 1,
                uuid: None,
                node_type: NodeType::Paragraph,
                parent_uuid: None,
                position: 1000.0,
                plain_text: "Hello.".to_string(),
                structural_hash: "h1".to_string(),
                has_content: true,
                char_start: None,
                char_end: None,
                heading_level: None,
                section_path: None,
                children: vec![StructuralNode {
                    version: 1,
                    uuid: None,
                    node_type: NodeType::Sentence,
                    parent_uuid: None,
                    position: 2000.0,
                    plain_text: "Hello.".to_string(),
                    structural_hash: "h2".to_string(),
                    has_content: false,
                    char_start: Some(0),
                    char_end: Some(6),
                    heading_level: None,
                    section_path: None,
                    children: vec![],
                    pandoc_ast_json: None,
                }],
                pandoc_ast_json: None,
            }],
            pandoc_ast_json: None,
        };

        assert_eq!(count_nodes(&root), 3); // Document + Paragraph + Sentence
        assert_eq!(count_sentences(&root), 1);
    }
}
