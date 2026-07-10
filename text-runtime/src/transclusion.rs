// ── Transclusion Edges ─────────────────────────────────────────────────────
//
// Typed directed edges between nodes across documents. Transclusions model
// compositional relationships: one node includes, cites, derives from, or
// otherwise references another.
//
// Predicates: "transcludes", "cites", "derives-from", "references",
//             "extends", "refutes", "translates", "summarizes"

use crate::error::TextRuntimeError;
use crate::store::db::DbStore;
use crate::store::types::TransclusionRow;
use crate::uuid7::uuid7;

/// Valid transclusion predicates.
const VALID_PREDICATES: &[&str] = &[
    "transcludes",
    "cites",
    "derives-from",
    "references",
    "extends",
    "refutes",
    "translates",
    "summarizes",
];

/// A transclusion edge between two nodes.
#[derive(Debug, Clone)]
pub struct Transclusion {
    pub uuid: String,
    pub predicate: String,
    pub source_node_uuid: String,
    pub source_doc_uuid: String,
    pub target_node_uuid: String,
    pub target_doc_uuid: String,
    pub status: String,
}

/// Create a transclusion edge between two nodes.
///
/// # Arguments
///
/// * `store` — The database store
/// * `source` — UUID of the source node (the one doing the including/citing)
/// * `target` — UUID of the target node (the one being included/cited)
/// * `predicate` — The relationship type. Must be one of the VALID_PREDICATES.
///
/// # Returns
///
/// The UUID of the newly created transclusion edge.
///
/// # Errors
///
/// * `InvalidPredicate` — if the predicate is not recognized
/// * `NodeNotFound` — if source or target node doesn't exist
/// * `CircularTransclusion` — if a cycle would be created (TODO)
pub fn create_transclusion(
    store: &DbStore,
    source: &str,
    target: &str,
    predicate: &str,
) -> Result<String, TextRuntimeError> {
    // Validate predicate
    if !VALID_PREDICATES.contains(&predicate) {
        return Err(TextRuntimeError::InvalidPredicate(
            predicate.to_string(),
            VALID_PREDICATES.join(", "),
        ));
    }

    // Verify source node exists
    let source_node = store.get_node(source)?;
    // Verify target node exists
    let target_node = store.get_node(target)?;

    // Check for self-reference
    if source == target {
        return Err(TextRuntimeError::CircularTransclusion(
            "source and target are the same node".to_string(),
        ));
    }

    let edge_uuid = uuid7().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    let row = TransclusionRow {
        id: 0,
        uuid: edge_uuid.clone(),
        predicate: predicate.to_string(),
        source_node_uuid: source.to_string(),
        source_doc_uuid: source_node.doc_id,
        target_node_uuid: target.to_string(),
        target_doc_uuid: target_node.doc_id,
        version_at_include: target_node.version,
        status: "live".to_string(),
        created_at: now,
    };

    store.insert_transclusion(&row)?;

    Ok(edge_uuid)
}

/// Get all transclusions where the given node is the source.
pub fn get_transclusions_by_source(
    store: &DbStore,
    node_uuid: &str,
) -> Result<Vec<Transclusion>, TextRuntimeError> {
    let rows = store.get_transclusions_for_source(node_uuid)?;
    Ok(rows.into_iter().map(row_to_transclusion).collect())
}

/// Get all transclusions where the given node is the target.
pub fn get_transclusions_by_target(
    store: &DbStore,
    node_uuid: &str,
) -> Result<Vec<Transclusion>, TextRuntimeError> {
    let rows = store.get_transclusions_for_target(node_uuid)?;
    Ok(rows.into_iter().map(row_to_transclusion).collect())
}

/// Check if a transclusion edge is stale (target node version changed).
///
/// Returns true if the target node's version has increased since the
/// transclusion was created, indicating the included content may
/// have changed.
pub fn check_staleness(
    store: &DbStore,
    transclusion: &Transclusion,
) -> Result<bool, TextRuntimeError> {
    let target_node = store.get_node(&transclusion.target_node_uuid)?;

    // Get the version_at_include from the DB row
    let rows = store.get_transclusions_for_source(&transclusion.source_node_uuid)?;
    let edge_row = rows
        .iter()
        .find(|r| r.uuid == transclusion.uuid)
        .ok_or_else(|| TextRuntimeError::TransclusionNotFound(transclusion.uuid.clone()))?;

    Ok(target_node.version > edge_row.version_at_include)
}

/// Detect and update stale transclusion edges.
///
/// Marks any stale transclusion edges as "stale" in the database.
pub fn refresh_transclusion_status(
    store: &DbStore,
    doc_id: Option<&str>,
) -> Result<usize, TextRuntimeError> {
    let stale_edges = store.detect_stale_edges(doc_id)?;
    let count = stale_edges.len();

    for edge in &stale_edges {
        store.update_transclusion_status(&edge.uuid, "stale")?;
    }

    Ok(count)
}

/// Convert a TransclusionRow to a Transclusion.
fn row_to_transclusion(row: TransclusionRow) -> Transclusion {
    Transclusion {
        uuid: row.uuid,
        predicate: row.predicate,
        source_node_uuid: row.source_node_uuid,
        source_doc_uuid: row.source_doc_uuid,
        target_node_uuid: row.target_node_uuid,
        target_doc_uuid: row.target_doc_uuid,
        status: row.status,
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::types::{DocumentRow, NodeRow};
    use tempfile::TempDir;

    fn setup_store() -> (DbStore, TempDir, String, String) {
        let tmp = TempDir::new().expect("temp dir");
        let db_path = tmp.path().join("test.sqlite");
        let store = DbStore::open(&db_path).expect("open db");

        let now = chrono::Utc::now().to_rfc3339();
        let doc_id = "00000000-0000-7000-8000-000000000001".to_string();
        let source_uuid = "00000000-0000-7000-8000-000000000002".to_string();
        let target_uuid = "00000000-0000-7000-8000-000000000003".to_string();

        store
            .insert_document(&DocumentRow {
                id: 0,
                uuid: doc_id.clone(),
                title: "Test Doc".to_string(),
                import_format: "markdown".to_string(),
                import_path: None,
                import_hash: None,
                root_node_uuid: Some(source_uuid.clone()),
                version: 1,
                ingested_at: now.clone(),
                language: "en".to_string(),
            })
            .expect("insert doc");

        let nodes = vec![
            NodeRow {
                id: 0,
                uuid: source_uuid.clone(),
                doc_id: doc_id.clone(),
                node_type: "paragraph".to_string(),
                parent_uuid: None,
                position: 1000.0,
                has_content: true,
                content_path: None,
                plain_text: "Source text.".to_string(),
                structural_hash: "hash1".to_string(),
                char_start: None,
                char_end: None,
                heading_level: None,
                section_path: None,
                version: 1,
                status: "active".to_string(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
            NodeRow {
                id: 0,
                uuid: target_uuid.clone(),
                doc_id: doc_id.clone(),
                node_type: "paragraph".to_string(),
                parent_uuid: None,
                position: 2000.0,
                has_content: true,
                content_path: None,
                plain_text: "Target text.".to_string(),
                structural_hash: "hash2".to_string(),
                char_start: None,
                char_end: None,
                heading_level: None,
                section_path: None,
                version: 1,
                status: "active".to_string(),
                created_at: now.clone(),
                updated_at: now,
            },
        ];

        for node in &nodes {
            store.insert_node(node).expect("insert node");
        }

        (store, tmp, source_uuid, target_uuid)
    }

    #[test]
    fn test_create_transclusion() {
        let (store, _tmp, source, target) = setup_store();

        let edge_uuid = create_transclusion(&store, &source, &target, "transcludes")
            .expect("create transclusion");

        assert!(!edge_uuid.is_empty());

        // Verify via source lookup
        let edges = get_transclusions_by_source(&store, &source).expect("get by source");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].predicate, "transcludes");
        assert_eq!(edges[0].source_node_uuid, source);
        assert_eq!(edges[0].target_node_uuid, target);
        assert_eq!(edges[0].status, "live");
    }

    #[test]
    fn test_create_transclusion_invalid_predicate() {
        let (store, _tmp, source, target) = setup_store();

        let result = create_transclusion(&store, &source, &target, "invalid-predicate");
        assert!(matches!(
            result,
            Err(TextRuntimeError::InvalidPredicate(_, _))
        ));
    }

    #[test]
    fn test_create_transclusion_self_reference() {
        let (store, _tmp, source, _target) = setup_store();

        let result = create_transclusion(&store, &source, &source, "cites");
        assert!(matches!(
            result,
            Err(TextRuntimeError::CircularTransclusion(_))
        ));
    }

    #[test]
    fn test_get_transclusions_by_target() {
        let (store, _tmp, source, target) = setup_store();

        create_transclusion(&store, &source, &target, "derives-from").expect("create transclusion");

        let edges = get_transclusions_by_target(&store, &target).expect("get by target");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].predicate, "derives-from");
    }

    #[test]
    fn test_all_valid_predicates_work() {
        let (store, _tmp, source, target) = setup_store();

        for predicate in VALID_PREDICATES {
            let result = create_transclusion(&store, &source, &target, predicate);
            assert!(result.is_ok(), "predicate '{}' should be valid", predicate);
        }
    }
}
