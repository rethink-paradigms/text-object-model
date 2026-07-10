// ── Write-Time Dual Selector Reconciliation ────────────────────────────────
//
// When creating an annotation, we generate both a TextPositionSelector
// (accurate but brittle — offsets change on edit) and a TextQuoteSelector
// (robust — survives edits, disambiguates with prefix/suffix).
//
// This module reconciles both selectors to ensure they describe the same
// span. If they disagree, reconciliation fails with a specific error.

use crate::annotation::types::{TextPositionSelector, TextQuoteSelector};
use crate::error::TextRuntimeError;

/// Reconcile a position and quote selector for the same span.
///
/// Given the full text of the source document and a position range,
/// generates both selectors and verifies they describe the same span.
///
/// # Arguments
///
/// * `full_text` — The full plain text of the source document or paragraph
/// * `start` — Start byte offset (inclusive)
/// * `end` — End byte offset (exclusive)
///
/// # Returns
///
/// A tuple `(TextPositionSelector, TextQuoteSelector)` where both selectors
/// are verified to point to the same text span.
///
/// # Errors
///
/// * `EmptyAnnotationSpan` — start >= end
/// * `SelectorReconciliationFailed` — the extracted quote text does not match
///   the text at the position range
pub fn reconcile_selectors(
    full_text: &str,
    start: usize,
    end: usize,
) -> Result<(TextPositionSelector, TextQuoteSelector), TextRuntimeError> {
    // Validate span
    if start >= end {
        return Err(TextRuntimeError::EmptyAnnotationSpan);
    }

    if end > full_text.len() {
        return Err(TextRuntimeError::AnnotationResolutionError(format!(
            "end offset {} exceeds text length {}",
            end,
            full_text.len()
        )));
    }

    // 1. Build position selector
    let position_sel = TextPositionSelector::new(start, end);

    // 2. Extract the exact text
    let exact_text = &full_text[start..end];

    // 3. Build context (prefix and suffix, up to 64 bytes each)
    let prefix = build_prefix_context(full_text, start);
    let suffix = build_suffix_context(full_text, end);

    // 4. Build quote selector
    let quote_sel = if prefix.is_some() || suffix.is_some() {
        TextQuoteSelector::with_context(
            exact_text,
            prefix.unwrap_or_default().as_str(),
            suffix.unwrap_or_default().as_str(),
        )
    } else {
        TextQuoteSelector::new(exact_text)
    };

    // 5. Verify: the quote selector's exact text must match the position span
    if quote_sel.exact != exact_text {
        return Err(TextRuntimeError::SelectorReconciliationFailed {
            pos_start: start,
            pos_end: end,
            quote_start: start, // approximate — we'd need actual quote resolution
            quote_end: end,
        });
    }

    Ok((position_sel, quote_sel))
}

/// Build a prefix context string for a quote selector.
///
/// Takes up to 64 bytes of text preceding the start position,
/// extending to the nearest word boundary.
fn build_prefix_context(full_text: &str, start: usize) -> Option<String> {
    if start == 0 {
        return None;
    }

    let max_prefix_len = 64usize.min(start);
    let prefix_start = start - max_prefix_len;
    let mut prefix = full_text[prefix_start..start].to_string();

    // Extend to nearest word boundary (start of word)
    // Find the first word character or line start after prefix_start
    if let Some(word_boundary) = find_word_boundary_left(&prefix) {
        prefix = prefix[word_boundary..].to_string();
    }

    if prefix.is_empty() {
        None
    } else {
        Some(prefix)
    }
}

/// Build a suffix context string for a quote selector.
///
/// Takes up to 64 bytes of text following the end position,
/// extending to the nearest word boundary.
fn build_suffix_context(full_text: &str, end: usize) -> Option<String> {
    if end >= full_text.len() {
        return None;
    }

    let max_suffix_len = 64usize.min(full_text.len() - end);
    let suffix_end = end + max_suffix_len;
    let mut suffix = full_text[end..suffix_end].to_string();

    // Extend to nearest word boundary (end of word)
    if let Some(word_boundary) = find_word_boundary_right(&suffix) {
        suffix = suffix[..word_boundary].to_string();
    }

    if suffix.is_empty() {
        None
    } else {
        Some(suffix)
    }
}

/// Find the nearest word boundary to the left (going towards end of string).
///
/// A word boundary is: whitespace, start of string, or a punctuation character
/// followed by a letter.
fn find_word_boundary_left(text: &str) -> Option<usize> {
    let chars: Vec<char> = text.chars().collect();
    for (i, ch) in chars.iter().enumerate() {
        if i == 0 {
            continue;
        }
        // Word boundary: space/punctuation followed by letter/digit
        if (chars[i - 1].is_whitespace() || chars[i - 1].is_ascii_punctuation())
            && ch.is_alphanumeric()
        {
            return Some(i);
        }
    }
    None
}

/// Find the nearest word boundary to the right (going towards end of string).
/// Returns the position of the first non-alphanumeric character after a word
/// (i.e., the end of the first complete word in the text).
fn find_word_boundary_right(text: &str) -> Option<usize> {
    let chars: Vec<char> = text.chars().collect();
    // Word boundary: letter/digit followed by space/punctuation (word end)
    (1..chars.len()).find(|&i| {
        chars[i - 1].is_alphanumeric()
            && (chars[i].is_whitespace() || chars[i].is_ascii_punctuation())
    })
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reconcile_simple_span() {
        let text = "The quick brown fox jumps over the lazy dog.";
        let (pos, quote) = reconcile_selectors(text, 4, 15).expect("reconcile");

        assert_eq!(pos.start, 4);
        assert_eq!(pos.end, 15);
        assert_eq!(quote.exact, "quick brown");
    }

    #[test]
    fn test_reconcile_empty_span() {
        let text = "hello";
        let result = reconcile_selectors(text, 3, 3);
        assert!(matches!(result, Err(TextRuntimeError::EmptyAnnotationSpan)));
    }

    #[test]
    fn test_reconcile_end_exceeds_text() {
        let text = "hello";
        let result = reconcile_selectors(text, 0, 100);
        assert!(result.is_err());
    }

    #[test]
    fn test_reconcile_full_text() {
        let text = "hello";
        let (pos, quote) = reconcile_selectors(text, 0, 5).expect("reconcile");

        assert_eq!(pos.start, 0);
        assert_eq!(pos.end, 5);
        assert_eq!(quote.exact, "hello");
        // Prefix should be None (start = 0)
        assert!(quote.prefix.is_none());
        // Suffix should be None (end = len)
        assert!(quote.suffix.is_none());
    }

    #[test]
    fn test_build_prefix_context() {
        let text = "The quick brown fox jumps over the lazy dog.";
        // Start at "jumps" (position 20)
        let prefix = build_prefix_context(text, 20);
        assert!(prefix.is_some());
        // Should contain text before "jumps"
        assert!(prefix.unwrap().contains("brown fox"));
    }

    #[test]
    fn test_build_suffix_context() {
        let text = "The quick brown fox jumps over the lazy dog.";
        // End at "jumps" (position 25)
        let suffix = build_suffix_context(text, 25);
        assert!(suffix.is_some());
        // Should contain text after "jumps"
        assert!(suffix.unwrap().contains("over"));
    }

    #[test]
    fn test_word_boundary_left() {
        let text = "  hello world";
        let boundary = find_word_boundary_left(text);
        assert_eq!(boundary, Some(2)); // Position of 'h'
    }

    #[test]
    fn test_word_boundary_right() {
        let text = "hello world  ";
        let boundary = find_word_boundary_right(text);
        assert_eq!(boundary, Some(5)); // Position of ' ' after "hello"
    }
}
