// ── Read-Time Re-Anchoring Cascade ────────────────────────────────────────
//
// When reading a document, annotations must be re-anchored to the current
// text state. The source document may have been edited since the annotation
// was created, so we use a cascade of strategies:
//
//   1. Fast path: Use position selector directly (text hasn't changed)
//   2. Quote-based search: Find exact text, disambiguate with prefix/suffix
//   3. Fallback: Report anchor status without exact position

use crate::annotation::types::Selector;

/// The status of an annotation's anchor after re-anchoring.
///
/// - `Active`: Both position and quote selectors agree, span is confirmed.
/// - `ActivePartial`: Quote matched but position shifted (text was edited).
/// - `Orphan`: Neither selector could be resolved — the text is gone.
/// - `Deleted`: The annotation has been explicitly deleted.
#[derive(Debug, Clone, PartialEq)]
pub enum AnchorStatus {
    /// Both selectors agree — the annotation is firmly anchored.
    Active,
    /// Quote matched but position shifted — text around the annotation changed.
    ActivePartial,
    /// Neither selector could be resolved — annotated text is gone.
    Orphan,
    /// Annotation was explicitly deleted by the user.
    Deleted,
}

impl AnchorStatus {
    /// Returns true if the annotation is at least partially anchored.
    pub fn is_active(&self) -> bool {
        matches!(self, AnchorStatus::Active | AnchorStatus::ActivePartial)
    }

    /// Returns a human-readable description of the status.
    pub fn as_str(&self) -> &'static str {
        match self {
            AnchorStatus::Active => "active",
            AnchorStatus::ActivePartial => "active_partial",
            AnchorStatus::Orphan => "orphan",
            AnchorStatus::Deleted => "deleted",
        }
    }
}

/// Resolve an annotation's span in the current full text using its selectors.
///
/// Uses a cascade of strategies:
///
/// 1. **Fast path (position)**: If a position selector exists and the text
///    at that position matches the quote selector's exact text, return
///    the position range immediately.
///
/// 2. **Quote-based search**: Search for the exact quote text in the full
///    text. If found once, return that span. If found multiple times,
///    disambiguate using prefix/suffix context.
///
/// 3. **Fallback**: Return None if neither strategy succeeds.
///
/// Returns `Some((start, end))` if the span could be resolved, or `None`
/// if the annotation is orphaned.
pub fn resolve_annotation_span(full_text: &str, selectors: &[Selector]) -> Option<(usize, usize)> {
    let pos_sel = selectors.iter().find_map(|s| s.position_range());
    let quote_sel = selectors.iter().find_map(|s| match s {
        Selector::TextQuote(q) => Some(q),
        _ => None,
    });

    // Strategy 1: Fast path — position selector matches quote
    if let Some((start, end)) = pos_sel {
        if let Some(quote) = quote_sel {
            // Verify the text at position matches the quote
            if start <= full_text.len() && end <= full_text.len() && start <= end {
                let actual_text = &full_text[start..end];
                if actual_text == quote.exact {
                    return Some((start, end));
                }
            }
        } else {
            // No quote selector — trust position if it's valid
            if start <= full_text.len() && end <= full_text.len() && start <= end {
                return Some((start, end));
            }
        }
    }

    // Strategy 2: Quote-based search
    if let Some(quote) = quote_sel {
        return find_quote_span(
            full_text,
            &quote.exact,
            quote.prefix.as_deref(),
            quote.suffix.as_deref(),
        );
    }

    // Strategy 3: Not found — orphan
    None
}

/// Search for a quote in the full text, disambiguating with prefix/suffix.
///
/// If the exact text appears exactly once, return its span directly.
/// If it appears multiple times, use prefix and suffix context to
/// disambiguate.
///
/// Returns None if the exact text cannot be found.
fn find_quote_span(
    full_text: &str,
    exact: &str,
    prefix: Option<&str>,
    suffix: Option<&str>,
) -> Option<(usize, usize)> {
    if exact.is_empty() {
        return None;
    }

    // Find all occurrences of the exact text
    let matches: Vec<usize> = full_text.match_indices(exact).map(|(i, _)| i).collect();

    match matches.len() {
        0 => None,
        1 => Some((matches[0], matches[0] + exact.len())),
        _ => {
            // Multiple matches — disambiguate
            disambiguate_matches(full_text, &matches, exact, prefix, suffix)
        }
    }
}

/// Disambiguate multiple matches using prefix and suffix context.
///
/// For each match position, check if the surrounding text matches the
/// prefix (text before the match) and/or suffix (text after).
///
/// Returns the best match, preferring the one that matches both prefix
/// and suffix, then prefix-only, then suffix-only.
fn disambiguate_matches(
    full_text: &str,
    positions: &[usize],
    _exact: &str,
    prefix: Option<&str>,
    suffix: Option<&str>,
) -> Option<(usize, usize)> {
    let exact_len = _exact.len();

    // Score each match: 2 = both prefix and suffix match, 1 = one matches, 0 = none
    let mut scored: Vec<(usize, usize, usize)> = positions
        .iter()
        .map(|&pos| {
            let mut score = 0usize;

            if let Some(p) = prefix {
                if !p.is_empty() && pos >= p.len() {
                    let actual_prefix = &full_text[pos - p.len()..pos];
                    if actual_prefix == p {
                        score += 1;
                    }
                }
            }

            if let Some(s) = suffix {
                if !s.is_empty() {
                    let suffix_start = pos + exact_len;
                    let suffix_end = (suffix_start + s.len()).min(full_text.len());
                    if suffix_end > suffix_start {
                        let actual_suffix = &full_text[suffix_start..suffix_end];
                        if actual_suffix == s {
                            score += 1;
                        }
                    }
                }
            }

            (pos, pos + exact_len, score)
        })
        .collect();

    // Sort by score descending
    scored.sort_by_key(|(_, _, score)| std::cmp::Reverse(*score));

    // Return the best match
    scored.first().map(|(start, end, _)| (*start, *end))
}

/// Determine the anchor status of annotations given the current text.
///
/// Takes the full text and the selectors, and returns both the resolved
/// span (if any) and the anchor status.
pub fn classify_anchor_status(
    full_text: &str,
    selectors: &[Selector],
) -> (AnchorStatus, Option<(usize, usize)>) {
    let pos_sel = selectors.iter().find_map(|s| s.position_range());
    let quote_sel = selectors.iter().find_map(|s| match s {
        Selector::TextQuote(q) => Some(q),
        _ => None,
    });

    // Check fast path (both selectors agree)
    if let Some((start, end)) = pos_sel {
        if start <= full_text.len() && end <= full_text.len() && start < end {
            let actual_text = &full_text[start..end];
            if let Some(quote) = quote_sel {
                if actual_text == quote.exact {
                    return (AnchorStatus::Active, Some((start, end)));
                }
            } else {
                return (AnchorStatus::Active, Some((start, end)));
            }
        }
    }

    // Check quote-based search
    if let Some(quote) = quote_sel {
        if let Some(span) = find_quote_span(
            full_text,
            &quote.exact,
            quote.prefix.as_deref(),
            quote.suffix.as_deref(),
        ) {
            // Quote found, but position may have shifted
            if let Some((p_start, p_end)) = pos_sel {
                if span != (p_start, p_end) {
                    return (AnchorStatus::ActivePartial, Some(span));
                }
            }
            return (AnchorStatus::ActivePartial, Some(span));
        }
    }

    // Neither strategy worked — orphan
    (AnchorStatus::Orphan, None)
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation::types::{TextPositionSelector, TextQuoteSelector};

    #[test]
    fn test_resolve_fast_path_position_and_quote_agree() {
        let text = "The quick brown fox jumps.";
        let selectors = vec![
            Selector::TextPosition(TextPositionSelector::new(4, 15)),
            Selector::TextQuote(TextQuoteSelector::new("quick brown")),
        ];

        let span = resolve_annotation_span(text, &selectors);
        assert_eq!(span, Some((4, 15)));
    }

    #[test]
    fn test_resolve_fast_path_position_only() {
        let text = "The quick brown fox jumps.";
        let selectors = vec![Selector::TextPosition(TextPositionSelector::new(4, 15))];

        let span = resolve_annotation_span(text, &selectors);
        assert_eq!(span, Some((4, 15)));
    }

    #[test]
    fn test_resolve_position_mismatch_falls_back_to_quote() {
        // Position is wrong (text was edited), but quote is findable
        let text = "The quick brown fox jumps.";
        let selectors = vec![
            Selector::TextPosition(TextPositionSelector::new(100, 200)),
            Selector::TextQuote(TextQuoteSelector::new("quick brown")),
        ];

        let span = resolve_annotation_span(text, &selectors);
        // Should still find via quote search
        assert_eq!(span, Some((4, 15)));
    }

    #[test]
    fn test_resolve_orphan_when_quote_not_found() {
        let text = "The quick brown fox jumps.";
        let selectors = vec![
            Selector::TextPosition(TextPositionSelector::new(4, 15)),
            Selector::TextQuote(TextQuoteSelector::new("deleted text")),
        ];

        let span = resolve_annotation_span(text, &selectors);
        assert_eq!(span, None);
    }

    #[test]
    fn test_classify_active() {
        let text = "Hello world.";
        let selectors = vec![Selector::position(0, 5), Selector::quote("Hello")];

        let (status, span) = classify_anchor_status(text, &selectors);
        assert_eq!(status, AnchorStatus::Active);
        assert_eq!(span, Some((0, 5)));
    }

    #[test]
    fn test_classify_active_partial() {
        let text = "Prefix Hello world.";
        let selectors = vec![
            Selector::position(0, 5), // wrong position
            Selector::quote("Hello"),
        ];

        let (status, span) = classify_anchor_status(text, &selectors);
        assert_eq!(status, AnchorStatus::ActivePartial);
        assert_eq!(span, Some((7, 12))); // actual position
    }

    #[test]
    fn test_classify_orphan() {
        let text = "Different text entirely.";
        let selectors = vec![Selector::position(0, 5), Selector::quote("missing")];

        let (status, span) = classify_anchor_status(text, &selectors);
        assert_eq!(status, AnchorStatus::Orphan);
        assert_eq!(span, None);
    }

    #[test]
    fn test_anchor_status_is_active() {
        assert!(AnchorStatus::Active.is_active());
        assert!(AnchorStatus::ActivePartial.is_active());
        assert!(!AnchorStatus::Orphan.is_active());
        assert!(!AnchorStatus::Deleted.is_active());
    }
}
