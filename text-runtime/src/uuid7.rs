// ── UUID v7 Allocator with Dedup ────────────────────────────────────────────
//
// Generates time-ordered UUID v7 values and tracks used UUIDs for
// collision-free allocation during ingestion.

use std::collections::HashSet;
use uuid::Uuid;

/// Generate a UUID v7 (time-ordered, monotonic within same millisecond).
///
/// UUID v7 embeds a 48-bit Unix millisecond timestamp in the most
/// significant bits, making UUIDs sortable by creation time.
#[inline]
pub fn uuid7() -> Uuid {
    Uuid::now_v7()
}

/// UUID allocator that tracks used IDs for deduplication.
///
/// Used during ingestion to ensure no collisions when assigning
/// UUIDs to new nodes. On re-ingestion, existing nodes are matched
/// by structural hash and keep their UUIDs; new nodes get fresh
/// UUIDs that are guaranteed not to collide with any existing UUID.
#[derive(Debug, Clone)]
pub struct UuidAllocator {
    used: HashSet<Uuid>,
}

impl UuidAllocator {
    /// Create a new empty allocator.
    pub fn new() -> Self {
        Self {
            used: HashSet::new(),
        }
    }

    /// Allocate a fresh UUID not already in the set.
    ///
    /// Generates UUID v7 values in a loop until one is found that
    /// is not in the used set. In practice, UUID v7 collisions are
    /// astronomically unlikely — this loop almost always terminates
    /// on the first iteration.
    pub fn allocate(&mut self) -> Uuid {
        loop {
            let candidate = uuid7();
            if self.used.insert(candidate) {
                return candidate;
            }
        }
    }

    /// Check if a UUID is already allocated.
    pub fn contains(&self, uuid: &Uuid) -> bool {
        self.used.contains(uuid)
    }

    /// Allocate a UUID, but if the given candidate already exists
    /// in the set, generate a new one instead (collision avoidance).
    ///
    /// If `candidate` is `None`, generates a fresh UUID.
    /// If `candidate` is `Some(u)` and `u` is not already used, inserts `u`
    /// and returns it. If `u` IS already used, generates a fresh UUID.
    pub fn allocate_dedup(&mut self, candidate: Option<Uuid>) -> Uuid {
        match candidate {
            Some(c) if !self.used.contains(&c) => {
                self.used.insert(c);
                c
            }
            _ => self.allocate(),
        }
    }
}

impl Default for UuidAllocator {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uuid7_format() {
        // UUID v7 has specific bit layout:
        // - Version nibble (bits 48-51) must be 0x7
        // - Variant bits (bits 64-65) must be 10
        let uuid = uuid7();
        let s = uuid.to_string();

        // Must be 36 chars with hyphens
        assert_eq!(s.len(), 36);
        assert_eq!(s.chars().filter(|&c| c == '-').count(), 4);

        // Character at position 14 (the version character) must be '7'
        assert_eq!(s.as_bytes()[14], b'7');

        // Character at position 19 must be '8', '9', 'a', or 'b' (variant 10xx)
        assert!(
            s.as_bytes()[19] == b'8'
                || s.as_bytes()[19] == b'9'
                || s.as_bytes()[19] == b'a'
                || s.as_bytes()[19] == b'b'
        );

        // Can parse back
        let parsed: Uuid = s.parse().expect("should parse back");
        assert_eq!(uuid, parsed);
        assert_eq!(parsed.get_version_num(), 7);
    }

    #[test]
    fn test_uuid7_monotonic() {
        // Two consecutive UUID v7s should sort by time.
        // In the same millisecond, the counter field ensures monotonic ordering.
        let u1 = uuid7();
        let u2 = uuid7();
        assert!(
            u1 <= u2,
            "uuid7 should be monotonic: {:?} <= {:?}",
            u1.as_u128(),
            u2.as_u128()
        );
    }

    #[test]
    fn test_dedup_10k() {
        // Allocate 10,000 UUIDs — every one must be unique.
        let mut allocator = UuidAllocator::new();
        let mut seen = HashSet::new();
        for _ in 0..10_000 {
            let u = allocator.allocate();
            assert!(seen.insert(u), "duplicate UUID allocated: {}", u);
        }
        assert_eq!(seen.len(), 10_000);
    }

    #[test]
    fn test_dedup_with_candidate() {
        let mut allocator = UuidAllocator::new();

        // First allocation — should accept the candidate
        let c1 = uuid7();
        let r1 = allocator.allocate_dedup(Some(c1));
        assert_eq!(r1, c1, "should accept unused candidate");

        // Second allocation with same candidate — should reject, allocate fresh
        let r2 = allocator.allocate_dedup(Some(c1));
        assert_ne!(r2, c1, "should reject duplicate candidate");

        // None candidate — generates fresh
        let r3 = allocator.allocate_dedup(None);
        assert!(allocator.contains(&r3));

        // All three are distinct
        assert_ne!(r1, r2);
        assert_ne!(r1, r3);
        assert_ne!(r2, r3);
    }
}
