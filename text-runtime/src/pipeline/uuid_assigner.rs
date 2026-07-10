// ── UUID Assigner ───────────────────────────────────────────────────────────
//
// Walks the structural tree and assigns UUIDs to every node.
// For each node:
//   1. Computes structural_hash = SHA-256 of normalized plain_text
//   2. Queries SQLite for existing nodes with the same hash in the same document
//   3. If found: reuses the existing UUID (keep identity)
//   4. If not found: allocates new UUID v7 from the allocator

use sha2::{Digest, Sha256};

use crate::error::TextRuntimeError;
use crate::store::db::DbStore;
use crate::types::{StructuralNode, Uuid};
use crate::uuid7::UuidAllocator;

/// Walk the structural tree and assign UUIDs to every node.
///
/// For each node:
/// - Compute structural_hash = SHA-256 of normalized plain_text
/// - Query SQLite for existing nodes with the same hash in the same document
/// - If found: reuse the existing UUID (keep identity)
/// - If not found: allocate new UUID v7 from the allocator
pub fn assign_uuids(
    root: &mut StructuralNode,
    doc_id: &str,
    db: &DbStore,
    allocator: &mut UuidAllocator,
) -> Result<(), TextRuntimeError> {
    assign_uuids_recursive(root, doc_id, db, allocator)
}

fn assign_uuids_recursive(
    node: &mut StructuralNode,
    doc_id: &str,
    db: &DbStore,
    allocator: &mut UuidAllocator,
) -> Result<(), TextRuntimeError> {
    // 1. Compute structural_hash if not already set
    if node.structural_hash.is_empty() {
        node.structural_hash = compute_structural_hash(&node.plain_text);
    }

    // 2. Try to find existing node by hash
    let existing = db.get_nodes_by_hash(doc_id, &node.structural_hash)?;

    if let Some(existing_node) = existing.first() {
        // Reuse the existing UUID — same content, same identity
        let existing_uuid: Uuid = existing_node
            .uuid
            .parse()
            .map_err(|_| TextRuntimeError::InvalidUuid(existing_node.uuid.clone()))?;
        node.uuid = Some(allocator.allocate_dedup(Some(existing_uuid)));
    } else {
        // No existing match — allocate fresh UUID
        let new_uuid = allocator.allocate();
        node.uuid = Some(new_uuid);
    }

    // 3. Recurse into children (setting their parent_uuid as we go)
    let my_uuid = node.uuid;
    for child in &mut node.children {
        child.parent_uuid = my_uuid;
        assign_uuids_recursive(child, doc_id, db, allocator)?;
    }

    Ok(())
}

/// Compute SHA-256 hash of normalized plain text (hex string, 64 chars).
pub fn compute_structural_hash(plain_text: &str) -> String {
    let normalized = plain_text.trim();
    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    format!("{:x}", hasher.finalize())
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_structural_hash_deterministic() {
        let h1 = compute_structural_hash("hello world");
        let h2 = compute_structural_hash("hello world");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn test_compute_structural_hash_different_for_different_text() {
        let h1 = compute_structural_hash("hello world");
        let h2 = compute_structural_hash("hello world!");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_compute_structural_hash_trims_whitespace() {
        let h1 = compute_structural_hash("  hello world  ");
        let h2 = compute_structural_hash("hello world");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_compute_structural_hash_empty() {
        let h = compute_structural_hash("");
        assert_eq!(h.len(), 64);
    }
}
