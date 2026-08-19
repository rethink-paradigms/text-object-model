// ── Re-Ingestion Diffing ────────────────────────────────────────────────────
//
// Compares new nodes against existing nodes from a prior ingest, preserving
// UUIDs where structural hashes match, fuzzy-matching where content has
// changed slightly, and marking deleted nodes that no longer exist.

use sha2::{Digest, Sha256};

use crate::error::TextRuntimeError;
use crate::store::db::DbStore;
use crate::store::types::NodeRow;
use crate::types::{StructuralNode, Uuid};
use crate::uuid7::UuidAllocator;

/// Compute SHA-256 hash of normalized plain text (hex string, 64 chars).
pub fn compute_structural_hash(plain_text: &str) -> String {
    let normalized = plain_text.trim();
    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Compute Levenshtein edit distance between two strings.
///
/// Uses a classic dynamic programming approach with O(n*m) time
/// and O(min(n,m)) space.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let a_len = a_chars.len();
    let b_len = b_chars.len();

    // Ensure b is the smaller string for space optimization
    if a_len < b_len {
        return edit_distance(b, a);
    }

    // Use two rows for DP
    let mut prev: Vec<usize> = (0..=b_len).collect();
    let mut curr: Vec<usize> = vec![0; b_len + 1];

    for i in 1..=a_len {
        curr[0] = i;
        for j in 1..=b_len {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };
            curr[j] = (prev[j] + 1) // deletion
                .min(curr[j - 1] + 1) // insertion
                .min(prev[j - 1] + cost); // substitution
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[b_len]
}

/// Check if edit distance is within threshold (as fraction of max length).
///
/// `threshold` is 0.0–1.0, default 0.20 (20%).
/// Returns true if `edit_distance(a, b) <= max(len(a), len(b)) * threshold`.
/// Two empty strings are always considered a fuzzy match.
pub fn is_fuzzy_match(a: &str, b: &str, threshold: f64) -> bool {
    let max_len = a.len().max(b.len());
    if max_len == 0 {
        return true;
    }
    edit_distance(a, b) <= (max_len as f64 * threshold) as usize
}

/// Full re-ingestion diff: compare new nodes against existing nodes,
/// preserve UUIDs where hashes match, fuzzy match with edit distance,
/// mark deleted nodes, orphan annotations.
///
/// # Arguments
///
/// * `new_root` - The newly segmented structural tree
/// * `existing_doc_id` - UUID of the existing document
/// * `existing_nodes` - All active nodes from the existing document
/// * `db` - Database store for lookups and updates
/// * `allocator` - UUID allocator for new nodes
/// * `threshold` - Fuzzy match threshold (0.0–1.0). Default 0.20 means
///   nodes with ≤20% edit distance keep their UUID.
///
/// # Returns
///
/// (kept_count, updated_count, deleted_count)
pub fn diff_and_merge(
    new_root: &mut StructuralNode,
    existing_doc_id: &str,
    existing_nodes: &[NodeRow],
    db: &mut DbStore,
    allocator: &mut UuidAllocator,
    threshold: f64,
) -> Result<(usize, usize, usize), TextRuntimeError> {
    let mut kept_count: usize = 0;
    let mut updated_count: usize = 0;
    let mut deleted_count: usize = 0;

    // Build a lookup: hash → existing_node
    let mut hash_to_existing: std::collections::HashMap<String, Vec<&NodeRow>> =
        std::collections::HashMap::new();
    for node in existing_nodes {
        hash_to_existing
            .entry(node.structural_hash.clone())
            .or_default()
            .push(node);
    }

    // Track which existing UUIDs were matched
    let mut matched_uuids: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Walk the new tree and match nodes
    diff_recursive(
        new_root,
        existing_doc_id,
        &hash_to_existing,
        &mut matched_uuids,
        db,
        allocator,
        threshold,
        &mut kept_count,
        &mut updated_count,
    )?;

    // Any existing UUIDs not matched → mark as deleted
    for node in existing_nodes {
        if !matched_uuids.contains(&node.uuid) {
            db.update_node_status(&node.uuid, "deleted")?;
            deleted_count += 1;
        }
    }

    // Mark annotations targeting deleted nodes as orphaned
    let deleted_uuids: Vec<String> = existing_nodes
        .iter()
        .filter(|n| !matched_uuids.contains(&n.uuid))
        .map(|n| n.uuid.clone())
        .collect();
    db.update_orphaned_annotations(&deleted_uuids)?;

    Ok((kept_count, updated_count, deleted_count))
}

// Recursion carries a lot of context; refactor to a context struct is
// tracked as future debt. (clippy::too_many_arguments)
#[allow(clippy::too_many_arguments)]
fn diff_recursive(
    node: &mut StructuralNode,
    _existing_doc_id: &str,
    hash_to_existing: &std::collections::HashMap<String, Vec<&NodeRow>>,
    matched_uuids: &mut std::collections::HashSet<String>,
    _db: &mut DbStore,
    allocator: &mut UuidAllocator,
    threshold: f64,
    kept_count: &mut usize,
    updated_count: &mut usize,
) -> Result<(), TextRuntimeError> {
    // Compute structural hash if not set
    if node.structural_hash.is_empty() {
        node.structural_hash = compute_structural_hash(&node.plain_text);
    }

    // Try exact hash match first
    let mut exact_match = None;
    if let Some(matches) = hash_to_existing.get(&node.structural_hash) {
        for existing in matches {
            if !matched_uuids.contains(&existing.uuid) {
                exact_match = Some(existing);
                break;
            }
        }
    }

    if let Some(existing) = exact_match {
        // Exact match — reuse UUID
        let uuid: Uuid = existing
            .uuid
            .parse()
            .map_err(|_| TextRuntimeError::InvalidUuid(existing.uuid.clone()))?;
        node.uuid = Some(allocator.allocate_dedup(Some(uuid)));
        node.version = existing.version;
        matched_uuids.insert(existing.uuid.clone());
        *kept_count += 1;
    } else {
        // No exact match — try fuzzy match
        let mut best_match: Option<(&NodeRow, f64)> = None;

        for nodes in hash_to_existing.values() {
            // Compute edit distance ratio
            for existing_node in nodes {
                if matched_uuids.contains(&existing_node.uuid) {
                    continue; // Already matched
                }

                let distance = edit_distance(&node.plain_text, &existing_node.plain_text);
                let max_len = node.plain_text.len().max(existing_node.plain_text.len());
                if max_len == 0 {
                    continue;
                }
                let ratio = distance as f64 / max_len as f64;

                if ratio <= threshold {
                    match best_match {
                        None => best_match = Some((existing_node, ratio)),
                        Some((_, best_ratio)) if ratio < best_ratio => {
                            best_match = Some((existing_node, ratio));
                        }
                        _ => {}
                    }
                }
            }
        }

        if let Some((best_node, _ratio)) = best_match {
            // Fuzzy match — reuse UUID, mark as updated
            let uuid: Uuid = best_node
                .uuid
                .parse()
                .map_err(|_| TextRuntimeError::InvalidUuid(best_node.uuid.clone()))?;
            node.uuid = Some(allocator.allocate_dedup(Some(uuid)));
            node.version = best_node.version + 1;
            matched_uuids.insert(best_node.uuid.clone());
            *updated_count += 1;
        } else {
            // No match — allocate new UUID
            assign_new_uuid(node, allocator);
        }
    }

    // Recurse into children
    for child in &mut node.children {
        diff_recursive(
            child,
            _existing_doc_id,
            hash_to_existing,
            matched_uuids,
            _db,
            allocator,
            threshold,
            kept_count,
            updated_count,
        )?;
    }

    Ok(())
}

fn assign_new_uuid(node: &mut StructuralNode, allocator: &mut UuidAllocator) {
    let new_uuid = allocator.allocate();
    node.uuid = Some(new_uuid);
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_same_text_same_hash() {
        let h1 = compute_structural_hash("hello world");
        let h2 = compute_structural_hash("hello world");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn test_different_text_different_hash() {
        let h1 = compute_structural_hash("hello world");
        let h2 = compute_structural_hash("goodbye world");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_levenshtein_kitten_sitting() {
        // "kitten" → "sitting" = 3
        // k→s, e→i, +g
        let d = edit_distance("kitten", "sitting");
        assert_eq!(d, 3);
    }

    #[test]
    fn test_levenshtein_same_string() {
        assert_eq!(edit_distance("hello", "hello"), 0);
    }

    #[test]
    fn test_levenshtein_empty() {
        assert_eq!(edit_distance("", ""), 0);
        assert_eq!(edit_distance("abc", ""), 3);
        assert_eq!(edit_distance("", "abc"), 3);
    }

    #[test]
    fn test_levenshtein_substitution() {
        // "cat" → "bat" = 1 (one substitution)
        assert_eq!(edit_distance("cat", "bat"), 1);
    }

    #[test]
    fn test_levenshtein_insertion() {
        // "cat" → "cats" = 1 (one insertion)
        assert_eq!(edit_distance("cat", "cats"), 1);
    }

    #[test]
    fn test_levenshtein_deletion() {
        // "cats" → "cat" = 1 (one deletion)
        assert_eq!(edit_distance("cats", "cat"), 1);
    }

    #[test]
    fn test_hash_deterministic_across_calls() {
        let h1 = compute_structural_hash("The quick brown fox");
        let h2 = compute_structural_hash("The quick brown fox");
        let h3 = compute_structural_hash("The quick brown fox");
        assert_eq!(h1, h2);
        assert_eq!(h2, h3);
    }

    #[test]
    fn test_hash_trims_whitespace() {
        let h1 = compute_structural_hash("  hello  ");
        let h2 = compute_structural_hash("hello");
        assert_eq!(h1, h2);
    }
    #[test]
    fn test_fuzzy_match_exact() {
        // 0 edit distance → always within any threshold
        assert!(is_fuzzy_match("hello", "hello", 0.0));
        assert!(is_fuzzy_match("hello", "hello", 0.20));
    }

    #[test]
    fn test_fuzzy_match_within_threshold() {
        // "cat" → "bat": edit distance 1, max_len 3, ratio 0.333...
        assert!(!is_fuzzy_match("cat", "bat", 0.20));
        assert!(is_fuzzy_match("cat", "bat", 0.34));
    }

    #[test]
    fn test_fuzzy_match_outside_threshold() {
        // "kitten" → "sitting": edit distance 3, max_len 7, ratio 0.428...
        assert!(!is_fuzzy_match("kitten", "sitting", 0.20));
        assert!(!is_fuzzy_match("kitten", "sitting", 0.40));
        assert!(is_fuzzy_match("kitten", "sitting", 0.43));
    }

    #[test]
    fn test_fuzzy_match_empty() {
        // Two empty strings
        assert!(is_fuzzy_match("", "", 0.0));
        assert!(is_fuzzy_match("", "", 0.20));

        // One empty, one not: edit distance = len of non-empty
        assert!(!is_fuzzy_match("", "abc", 0.20));
        // 3 edits, max_len 3, ratio 1.0 → needs threshold >= 1.0
        assert!(is_fuzzy_match("", "abc", 1.0));
    }

    #[test]
    fn test_fuzzy_match_zero_threshold() {
        // threshold 0.0 → only exact matches allowed
        assert!(is_fuzzy_match("abc", "abc", 0.0));
        assert!(!is_fuzzy_match("abc", "abd", 0.0));
        assert!(is_fuzzy_match("", "", 0.0));
    }
}
