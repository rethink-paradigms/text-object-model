// ── Navigation Queries ──────────────────────────────────────────────────────
// Pure navigation functions that operate on an existing DbStore connection.
// All queries use parameterized statements. No unwrap().

use crate::error::TextRuntimeError;
use crate::store::db::DbStore;
use crate::store::types::NodeRow;

/// Get the parent UUID of a node.
///
/// Returns `None` if the node has no parent (e.g., document root).
pub fn parent(store: &DbStore, uuid: &str) -> Result<Option<String>, TextRuntimeError> {
    let node = store.get_node(uuid)?;
    Ok(node.parent_uuid)
}

/// Get child UUIDs of a node, ordered by position.
pub fn children(store: &DbStore, uuid: &str) -> Result<Vec<String>, TextRuntimeError> {
    let rows = store.get_children(uuid)?;
    Ok(rows.into_iter().map(|n| n.uuid).collect())
}

/// Get sibling UUIDs (children of the same parent, excluding self).
///
/// Returns an empty vector if the node has no parent.
pub fn siblings(store: &DbStore, uuid: &str) -> Result<Vec<String>, TextRuntimeError> {
    let node = store.get_node(uuid)?;
    let parent_uuid = match &node.parent_uuid {
        Some(p) => p.clone(),
        None => return Ok(Vec::new()),
    };
    let rows = store.get_children(&parent_uuid)?;
    Ok(rows
        .into_iter()
        .filter(|n| n.uuid != uuid)
        .map(|n| n.uuid)
        .collect())
}

/// Get the previous sibling (immediately before in parent's children by position).
///
/// Returns `None` if this is the first child or the node has no parent.
pub fn prev(store: &DbStore, uuid: &str) -> Result<Option<String>, TextRuntimeError> {
    let node = store.get_node(uuid)?;
    let parent_uuid = match &node.parent_uuid {
        Some(p) => p.clone(),
        None => return Ok(None),
    };
    let rows = store.get_children(&parent_uuid)?;
    let mut prev_uuid: Option<String> = None;
    for child in &rows {
        if child.uuid == uuid {
            return Ok(prev_uuid);
        }
        prev_uuid = Some(child.uuid.clone());
    }
    Ok(None)
}

/// Get the next sibling (immediately after in parent's children by position).
///
/// Returns `None` if this is the last child or the node has no parent.
pub fn next(store: &DbStore, uuid: &str) -> Result<Option<String>, TextRuntimeError> {
    let node = store.get_node(uuid)?;
    let parent_uuid = match &node.parent_uuid {
        Some(p) => p.clone(),
        None => return Ok(None),
    };
    let rows = store.get_children(&parent_uuid)?;
    let mut found = false;
    for child in &rows {
        if found {
            return Ok(Some(child.uuid.clone()));
        }
        if child.uuid == uuid {
            found = true;
        }
    }
    Ok(None)
}

/// Get table of contents: all headings in a document, ordered by position.
pub fn toc(store: &DbStore, doc_id: &str) -> Result<Vec<NodeRow>, TextRuntimeError> {
    // Verify document exists
    let _ = store.get_document(doc_id)?;

    let all_nodes = store.get_nodes_by_doc(doc_id)?;
    Ok(all_nodes
        .into_iter()
        .filter(|n| n.node_type == "heading")
        .collect())
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::types::{DocumentRow, NodeRow};

    use tempfile::TempDir;

    /// Create a temporary DbStore with a test document and several nodes.
    fn setup_store() -> (DbStore, TempDir, String, Vec<String>) {
        let tmp = TempDir::new().expect("temp dir");
        let db_path = tmp.path().join("test.sqlite");
        let store = DbStore::open(&db_path).expect("open db");

        let now = chrono::Utc::now().to_rfc3339();
        let doc_id = "00000000-0000-7000-8000-000000000001".to_string();
        let root_uuid = "00000000-0000-7000-8000-000000000002".to_string();
        let h1_uuid = "00000000-0000-7000-8000-000000000003".to_string();
        let p1_uuid = "00000000-0000-7000-8000-000000000004".to_string();
        let h2_uuid = "00000000-0000-7000-8000-000000000005".to_string();
        let p2_uuid = "00000000-0000-7000-8000-000000000006".to_string();

        // Insert document
        store
            .insert_document(&DocumentRow {
                id: 0,
                uuid: doc_id.clone(),
                title: "Test Doc".to_string(),
                import_format: "markdown".to_string(),
                import_path: None,
                import_hash: None,
                root_node_uuid: Some(root_uuid.clone()),
                version: 1,
                ingested_at: now.clone(),
                language: "en".to_string(),
            })
            .expect("insert document");

        let node_uuids = vec![
            root_uuid.clone(),
            h1_uuid.clone(),
            p1_uuid.clone(),
            h2_uuid.clone(),
            p2_uuid.clone(),
        ];

        // Insert nodes
        let nodes = vec![
            make_node(&root_uuid, &doc_id, "document", None, 1000.0, "", &now),
            make_node(
                &h1_uuid,
                &doc_id,
                "heading",
                Some(&root_uuid),
                2000.0,
                "Introduction",
                &now,
            ),
            make_node(
                &p1_uuid,
                &doc_id,
                "paragraph",
                Some(&root_uuid),
                3000.0,
                "Hello world.",
                &now,
            ),
            make_node(
                &h2_uuid,
                &doc_id,
                "heading",
                Some(&root_uuid),
                4000.0,
                "Conclusion",
                &now,
            ),
            make_node(
                &p2_uuid,
                &doc_id,
                "paragraph",
                Some(&root_uuid),
                5000.0,
                "Goodbye.",
                &now,
            ),
        ];

        for node in &nodes {
            store.insert_node(node).expect("insert node");
        }

        (store, tmp, doc_id, node_uuids)
    }

    fn make_node(
        uuid: &str,
        doc_id: &str,
        node_type: &str,
        parent_uuid: Option<&str>,
        position: f64,
        plain_text: &str,
        now: &str,
    ) -> NodeRow {
        NodeRow {
            id: 0,
            uuid: uuid.to_string(),
            doc_id: doc_id.to_string(),
            node_type: node_type.to_string(),
            parent_uuid: parent_uuid.map(|s| s.to_string()),
            position,
            has_content: false,
            content_path: None,
            plain_text: plain_text.to_string(),
            structural_hash: "abc123".to_string(),
            char_start: None,
            char_end: None,
            heading_level: if node_type == "heading" {
                Some(1)
            } else {
                None
            },
            section_path: None,
            version: 1,
            status: "active".to_string(),
            created_at: now.to_string(),
            updated_at: now.to_string(),
        }
    }

    // ── parent ──────────────────────────────────────────────────────────

    #[test]
    fn test_parent_returns_parent_uuid() {
        let (store, _tmp, _doc_id, uuids) = setup_store();
        // h1's parent is root
        let p = parent(&store, &uuids[1]).expect("parent");
        assert_eq!(p, Some(uuids[0].clone()));
    }

    #[test]
    fn test_parent_returns_none_for_root() {
        let (store, _tmp, _doc_id, uuids) = setup_store();
        let p = parent(&store, &uuids[0]).expect("parent");
        assert_eq!(p, None);
    }

    // ── children ────────────────────────────────────────────────────────

    #[test]
    fn test_children_returns_ordered_uuids() {
        let (store, _tmp, _doc_id, uuids) = setup_store();
        let kids = children(&store, &uuids[0]).expect("children");
        // Should be: h1, p1, h2, p2 (4 children, ordered by position)
        assert_eq!(kids.len(), 4);
        assert_eq!(kids[0], uuids[1]); // h1
        assert_eq!(kids[1], uuids[2]); // p1
        assert_eq!(kids[2], uuids[3]); // h2
        assert_eq!(kids[3], uuids[4]); // p2
    }

    #[test]
    fn test_children_returns_empty_for_leaf() {
        let (store, _tmp, _doc_id, uuids) = setup_store();
        let kids = children(&store, &uuids[2]).expect("children");
        assert!(kids.is_empty());
    }

    // ── siblings ────────────────────────────────────────────────────────

    #[test]
    fn test_siblings_excludes_self() {
        let (store, _tmp, _doc_id, uuids) = setup_store();
        // h1's siblings: p1, h2, p2 (not h1 itself)
        let sibs = siblings(&store, &uuids[1]).expect("siblings");
        assert_eq!(sibs.len(), 3);
        assert!(!sibs.contains(&uuids[1]));
        assert!(sibs.contains(&uuids[2])); // p1
        assert!(sibs.contains(&uuids[3])); // h2
        assert!(sibs.contains(&uuids[4])); // p2
    }

    #[test]
    fn test_siblings_returns_empty_for_root() {
        let (store, _tmp, _doc_id, uuids) = setup_store();
        let sibs = siblings(&store, &uuids[0]).expect("siblings");
        assert!(sibs.is_empty());
    }

    // ── prev ────────────────────────────────────────────────────────────

    #[test]
    fn test_prev_returns_previous_sibling() {
        let (store, _tmp, _doc_id, uuids) = setup_store();
        // p1's prev is h1
        let p = prev(&store, &uuids[2]).expect("prev");
        assert_eq!(p, Some(uuids[1].clone()));
    }

    #[test]
    fn test_prev_returns_none_for_first_child() {
        let (store, _tmp, _doc_id, uuids) = setup_store();
        // h1 is first child
        let p = prev(&store, &uuids[1]).expect("prev");
        assert_eq!(p, None);
    }

    #[test]
    fn test_prev_returns_none_for_root() {
        let (store, _tmp, _doc_id, uuids) = setup_store();
        let p = prev(&store, &uuids[0]).expect("prev");
        assert_eq!(p, None);
    }

    // ── next ────────────────────────────────────────────────────────────

    #[test]
    fn test_next_returns_next_sibling() {
        let (store, _tmp, _doc_id, uuids) = setup_store();
        // h1's next is p1
        let n = next(&store, &uuids[1]).expect("next");
        assert_eq!(n, Some(uuids[2].clone()));
    }

    #[test]
    fn test_next_returns_none_for_last_child() {
        let (store, _tmp, _doc_id, uuids) = setup_store();
        // p2 is last child
        let n = next(&store, &uuids[4]).expect("next");
        assert_eq!(n, None);
    }

    #[test]
    fn test_next_returns_none_for_root() {
        let (store, _tmp, _doc_id, uuids) = setup_store();
        let n = next(&store, &uuids[0]).expect("next");
        assert_eq!(n, None);
    }

    // ── toc ─────────────────────────────────────────────────────────────

    #[test]
    fn test_toc_returns_only_headings() {
        let (store, _tmp, doc_id, uuids) = setup_store();
        let headings = toc(&store, &doc_id).expect("toc");
        assert_eq!(headings.len(), 2);
        assert_eq!(headings[0].uuid, uuids[1]); // h1
        assert_eq!(headings[1].uuid, uuids[3]); // h2
        for h in &headings {
            assert_eq!(h.node_type, "heading");
        }
    }

    #[test]
    fn test_toc_returns_empty_for_no_headings() {
        let tmp = TempDir::new().expect("temp dir");
        let db_path = tmp.path().join("test.sqlite");
        let store = DbStore::open(&db_path).expect("open db");

        let now = chrono::Utc::now().to_rfc3339();
        let doc_id = "00000000-0000-7000-8000-000000000010".to_string();
        let root_uuid = "00000000-0000-7000-8000-000000000011".to_string();

        store
            .insert_document(&DocumentRow {
                id: 0,
                uuid: doc_id.clone(),
                title: "No Headings".to_string(),
                import_format: "plain".to_string(),
                import_path: None,
                import_hash: None,
                root_node_uuid: Some(root_uuid.clone()),
                version: 1,
                ingested_at: now.clone(),
                language: "en".to_string(),
            })
            .expect("insert document");

        store
            .insert_node(&make_node(
                &root_uuid, &doc_id, "document", None, 1000.0, "", &now,
            ))
            .expect("insert node");

        let headings = toc(&store, &doc_id).expect("toc");
        assert!(headings.is_empty());
    }
}
