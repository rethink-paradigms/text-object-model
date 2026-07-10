// ── SQLite Writer ───────────────────────────────────────────────────────────
//
// Inserts (or updates) document + nodes into SQLite in a single transaction.
// Walks the structural tree and creates DocumentRow + NodeRow for each node.

use crate::error::TextRuntimeError;
use crate::store::db::DbStore;
use crate::store::types::{DocumentRow, NodeRow};
use crate::types::StructuralNode;

/// Insert (or update) document + nodes into SQLite in a single transaction.
///
/// For the document: INSERT OR REPLACE into documents table.
/// For each node: INSERT into nodes table.
///
/// Returns Vec of NodeRow for all inserted nodes and Vec of NodeRow for all
/// updated nodes.
pub fn write_to_sqlite(
    db: &mut DbStore,
    doc: &DocumentRow,
    root: &StructuralNode,
) -> Result<(Vec<NodeRow>, Vec<NodeRow>), TextRuntimeError> {
    let mut new_nodes: Vec<NodeRow> = Vec::new();
    let updated_nodes: Vec<NodeRow> = Vec::new();

    // Check document existence before insert
    let doc_exists = db.get_document(&doc.uuid).is_ok();

    if doc_exists {
        db.update_document_version(&doc.uuid, doc.version)?;
    } else {
        db.insert_document(doc)?;
    }

    // Walk the tree and insert nodes (WAL mode provides crash safety)
    let now = chrono::Utc::now().to_rfc3339();
    collect_and_insert_nodes(db, root, &doc.uuid, &now, &mut new_nodes)?;

    Ok((new_nodes, updated_nodes))
}

fn collect_and_insert_nodes(
    db: &DbStore,
    node: &StructuralNode,
    doc_id: &str,
    now: &str,
    new_nodes: &mut Vec<NodeRow>,
) -> Result<(), TextRuntimeError> {
    let uuid_str = node.uuid.map(|u| u.to_string()).unwrap_or_default();

    if uuid_str.is_empty() {
        return Err(TextRuntimeError::InternalError(
            "node has no UUID assigned".to_string(),
        ));
    }

    let node_row = NodeRow {
        id: 0,
        uuid: uuid_str.clone(),
        doc_id: doc_id.to_string(),
        node_type: node.node_type.as_str().to_string(),
        parent_uuid: node.parent_uuid.map(|u| u.to_string()),
        position: node.position,
        has_content: node.has_content,
        content_path: if node.has_content {
            Some(format!("{}/{}.json", &uuid_str[..2], uuid_str))
        } else {
            None
        },
        plain_text: node.plain_text.clone(),
        structural_hash: node.structural_hash.clone(),
        char_start: node.char_start.map(|c| c as i64),
        char_end: node.char_end.map(|c| c as i64),
        heading_level: node.heading_level,
        section_path: node.section_path.clone(),
        version: node.version,
        status: "active".to_string(),
        created_at: now.to_string(),
        updated_at: now.to_string(),
    };

    // Insert the node directly; re-ingestion dedup is handled upstream
    db.insert_node(&node_row)?;
    new_nodes.push(node_row);

    // Recurse into children
    for child in &node.children {
        collect_and_insert_nodes(db, child, doc_id, now, new_nodes)?;
    }

    Ok(())
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use crate::types::NodeType;
    use crate::uuid7::UuidAllocator;
    use tempfile::TempDir;

    fn setup_store() -> (Store, TempDir) {
        let tmp = TempDir::new().expect("temp dir");
        let runtime_dir = tmp.path().join(".textruntime");
        let store = Store::open(&runtime_dir).expect("open store");
        (store, tmp)
    }

    fn make_test_node() -> StructuralNode {
        let mut allocator = UuidAllocator::new();
        StructuralNode {
            version: 1,
            uuid: Some(allocator.allocate()),
            node_type: NodeType::Paragraph,
            parent_uuid: None,
            position: 1000.0,
            plain_text: "Hello world.".to_string(),
            structural_hash: "abc123def456".to_string(),
            has_content: true,
            char_start: None,
            char_end: None,
            heading_level: None,
            section_path: None,
            children: Vec::new(),
            pandoc_ast_json: Some(serde_json::json!({"t": "Para", "c": []})),
        }
    }

    #[test]
    fn test_write_to_sqlite_inserts_document_and_nodes() {
        let (mut store, _tmp) = setup_store();
        let root = make_test_node();
        let doc_uuid = crate::uuid7::uuid7().to_string();
        let now = chrono::Utc::now().to_rfc3339();

        let doc = DocumentRow {
            id: 0,
            uuid: doc_uuid.clone(),
            title: "Test Doc".to_string(),
            import_format: "markdown".to_string(),
            import_path: None,
            import_hash: None,
            root_node_uuid: root.uuid.map(|u| u.to_string()),
            version: 1,
            ingested_at: now.clone(),
            language: "en".to_string(),
        };

        let (new_nodes, updated_nodes) =
            write_to_sqlite(&mut store.db, &doc, &root).expect("write to sqlite");

        assert!(!new_nodes.is_empty(), "should have new nodes");
        assert_eq!(
            updated_nodes.len(),
            0,
            "should have no updated nodes on first insert"
        );

        // Verify document was inserted
        let retrieved_doc = store.db.get_document(&doc_uuid).expect("get document");
        assert_eq!(retrieved_doc.title, "Test Doc");

        // Verify node was inserted
        let node_uuid = root.uuid.unwrap().to_string();
        let retrieved_node = store.db.get_node(&node_uuid).expect("get node");
        assert_eq!(retrieved_node.plain_text, "Hello world.");
    }

    #[test]
    fn test_write_to_sqlite_nested_nodes() {
        let (mut store, _tmp) = setup_store();
        let mut allocator = UuidAllocator::new();
        let now = chrono::Utc::now().to_rfc3339();

        let doc_uuid = allocator.allocate().to_string();

        // Build a tree: paragraph → sentence1, sentence2
        let mut para = StructuralNode {
            version: 1,
            uuid: Some(allocator.allocate()),
            node_type: NodeType::Paragraph,
            parent_uuid: None,
            position: 1000.0,
            plain_text: "Hello world. This is fine.".to_string(),
            structural_hash: "hash_para".to_string(),
            has_content: true,
            char_start: None,
            char_end: None,
            heading_level: None,
            section_path: None,
            children: vec![
                StructuralNode {
                    version: 1,
                    uuid: Some(allocator.allocate()),
                    node_type: NodeType::Sentence,
                    parent_uuid: None, // will be filled
                    position: 2000.0,
                    plain_text: "Hello world.".to_string(),
                    structural_hash: "hash_s1".to_string(),
                    has_content: false,
                    char_start: Some(0),
                    char_end: Some(12),
                    heading_level: None,
                    section_path: None,
                    children: vec![],
                    pandoc_ast_json: None,
                },
                StructuralNode {
                    version: 1,
                    uuid: Some(allocator.allocate()),
                    node_type: NodeType::Sentence,
                    parent_uuid: None,
                    position: 3000.0,
                    plain_text: "This is fine.".to_string(),
                    structural_hash: "hash_s2".to_string(),
                    has_content: false,
                    char_start: Some(13),
                    char_end: Some(26),
                    heading_level: None,
                    section_path: None,
                    children: vec![],
                    pandoc_ast_json: None,
                },
            ],
            pandoc_ast_json: Some(serde_json::json!({"t": "Para", "c": []})),
        };

        // Set parent_uuid for children
        let para_uuid = para.uuid;
        for child in &mut para.children {
            child.parent_uuid = para_uuid;
        }

        let doc = DocumentRow {
            id: 0,
            uuid: doc_uuid.clone(),
            title: "Nested Test".to_string(),
            import_format: "markdown".to_string(),
            import_path: None,
            import_hash: None,
            root_node_uuid: para_uuid.map(|u| u.to_string()),
            version: 1,
            ingested_at: now.clone(),
            language: "en".to_string(),
        };

        let (new_nodes, _updated) =
            write_to_sqlite(&mut store.db, &doc, &para).expect("write to sqlite");

        // Should have 3 new nodes: paragraph + 2 sentences
        assert_eq!(new_nodes.len(), 3, "should have 3 new nodes");

        // Verify sentences have parent_uuid set
        for child in &para.children {
            let child_uuid = child.uuid.unwrap().to_string();
            let retrieved = store.db.get_node(&child_uuid).expect("get sentence");
            assert!(
                retrieved.parent_uuid.is_some(),
                "sentence should have parent"
            );
            assert!(
                !retrieved.has_content,
                "sentence should have has_content=false"
            );
            assert!(
                retrieved.char_start.is_some(),
                "sentence should have char_start"
            );
            assert!(
                retrieved.char_end.is_some(),
                "sentence should have char_end"
            );
        }
    }
}
