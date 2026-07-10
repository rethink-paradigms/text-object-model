// ── Content File Writer ─────────────────────────────────────────────────────
//
// Writes Pandoc AST fragment JSON to content files for all has_content=true
// nodes using the atomic write protocol.

use crate::error::TextRuntimeError;
use crate::store::content::ContentStore;
use crate::types::StructuralNode;

/// Write Pandoc AST fragment JSON to content files for all has_content=true
/// nodes in the structural tree.
///
/// Uses ContentStore's atomic write protocol: tmp/{uuid}.tmp → fsync →
/// rename to content/{first2}/{uuid}.json.
///
/// Sentence nodes are skipped (has_content=false — they derive from parent).
/// Section/document nodes are also skipped (has_content=false).
pub fn write_content_files(
    root: &StructuralNode,
    content_store: &ContentStore,
) -> Result<(), TextRuntimeError> {
    write_content_files_recursive(root, content_store)
}

fn write_content_files_recursive(
    node: &StructuralNode,
    content_store: &ContentStore,
) -> Result<(), TextRuntimeError> {
    // Only write content files for nodes that:
    // 1. Have has_content = true
    // 2. Have a UUID assigned
    // 3. Have pandoc_ast_json content
    if node.has_content {
        if let Some(uuid) = &node.uuid {
            if let Some(ast_json) = &node.pandoc_ast_json {
                let json_bytes = serde_json::to_vec_pretty(ast_json)
                    .map_err(TextRuntimeError::SerializationError)?;
                content_store.put(uuid, &json_bytes)?;
            }
        }
    }

    // Recurse into children
    for child in &node.children {
        write_content_files_recursive(child, content_store)?;
    }

    Ok(())
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{NodeType, Uuid};

    use tempfile::TempDir;

    fn make_test_node(has_content: bool, uuid: Option<Uuid>) -> StructuralNode {
        StructuralNode {
            version: 1,
            uuid,
            node_type: if has_content {
                NodeType::Paragraph
            } else {
                NodeType::Sentence
            },
            parent_uuid: None,
            position: 1000.0,
            plain_text: "test".to_string(),
            structural_hash: "abc123".to_string(),
            has_content,
            char_start: None,
            char_end: None,
            heading_level: None,
            section_path: None,
            children: Vec::new(),
            pandoc_ast_json: if has_content {
                Some(serde_json::json!({"t": "Para", "c": []}))
            } else {
                None
            },
        }
    }

    #[test]
    fn test_write_content_files_skips_sentences() {
        let tmp = TempDir::new().expect("temp dir");
        let content_path = tmp.path().join("content");
        let store = ContentStore::new(content_path).expect("create store");

        let sentence_node = make_test_node(false, Some(crate::uuid7::uuid7()));
        write_content_files(&sentence_node, &store).expect("write content files");

        // Sentence should NOT have a content file
        if let Some(uuid) = &sentence_node.uuid {
            assert!(!store.exists(uuid), "sentence should not have content file");
        }
    }

    #[test]
    fn test_write_content_files_writes_block_nodes() {
        let tmp = TempDir::new().expect("temp dir");
        let content_path = tmp.path().join("content");
        let store = ContentStore::new(content_path).expect("create store");

        let para_node = make_test_node(true, Some(crate::uuid7::uuid7()));
        write_content_files(&para_node, &store).expect("write content files");

        // Paragraph should have a content file
        if let Some(uuid) = &para_node.uuid {
            assert!(store.exists(uuid), "paragraph should have content file");

            let content = store.get(uuid).expect("get content");
            let json_str = String::from_utf8_lossy(&content);
            assert!(json_str.contains("Para"), "content should be Para JSON");
        }
    }

    #[test]
    fn test_write_content_files_nested() {
        let tmp = TempDir::new().expect("temp dir");
        let content_path = tmp.path().join("content");
        let store = ContentStore::new(content_path).expect("create store");

        let mut parent = make_test_node(true, Some(crate::uuid7::uuid7()));
        let child = make_test_node(false, Some(crate::uuid7::uuid7()));
        parent.children.push(child);

        write_content_files(&parent, &store).expect("write content files");

        // Parent should have file
        if let Some(uuid) = &parent.uuid {
            assert!(store.exists(uuid));
        }
        // Child (sentence) should not
        if let Some(uuid) = &parent.children[0].uuid {
            assert!(!store.exists(uuid));
        }
    }
}
