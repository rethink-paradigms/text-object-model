// ── Annotation Module ──────────────────────────────────────────────────────
//
// W3C Web Annotation system for the Text Runtime.
//
// Provides:
//   - `types.rs` — serde types for W3C JSON-LD annotations
//   - `reconcile.rs` — write-time dual selector reconciliation
//   - `anchoring.rs` — read-time re-anchoring cascade
//
// All annotation operations are idempotent and lossless.

pub mod anchoring;
pub mod reconcile;
pub mod types;

pub use anchoring::{resolve_annotation_span, AnchorStatus};
pub use reconcile::reconcile_selectors;
pub use types::*;
