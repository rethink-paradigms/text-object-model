// ── Inline-to-Offset Mapping ────────────────────────────────────────────────
//
// THE critical algorithm: walks a Pandoc inline array, extracts flat plain
// text, and builds a position index that maps every byte offset in the flat
// text back to positions in the inline tree.
//
// Handles ALL 20 Pandoc Inline variants classified into 5 types (A-E).
//
// TYPE_A (contributes text, CAN split): Str, Space, SoftBreak, LineBreak
// TYPE_B (contributes text, CANNOT split — atomic): Code, Math, RawInline
// TYPE_C (container, recurse): Emph, Strong, Underline, Strikeout,
//         Superscript, Subscript, SmallCaps, Quoted, Span, Link, Cite
// TYPE_D (skip entirely): Note
// TYPE_E (atomic caption): Image

use pandoc_ast::Inline;

use crate::types::{SentenceSpan, TextOffsetInInline, TextPosition};

// ── Public API ──────────────────────────────────────────────────────────────

/// Extract flat plain text from a Pandoc inline array, building a position
/// index that maps every byte offset in the flat text back to positions in
/// the inline tree.
///
/// Returns (flat_text, position_index).
///
/// position_index is a Vec<TextPosition> where entries are non-overlapping
/// and cover the entire flat_text in order.
pub fn extract_text_with_positions(inlines: &[Inline]) -> (String, Vec<TextPosition>) {
    let mut flat = String::new();
    let mut positions: Vec<TextPosition> = Vec::new();
    // Stack of (inline_index, container_start_offset_in_flat, inline_kind)
    // container_start_offset: the flat position where this container's text started
    let mut container_stack: Vec<(usize, usize, &'static str)> = Vec::new();

    walk_inlines(
        inlines,
        &mut flat,
        &mut positions,
        &mut container_stack,
        false,
    );
    (flat, positions)
}

/// Map icu_segmenter sentence breakpoints to sentence spans, respecting
/// atomic inline boundaries. If a breakpoint falls inside an atomic range
/// (Code, Math, RawInline, Image), it is pushed to the nearest safe boundary.
///
/// Returns Vec<SentenceSpan> with char_start/char_end byte offsets.
pub fn map_sentence_boundaries(
    _flat_text: &str,
    positions: &[TextPosition],
    breakpoints: &[usize],
) -> Vec<SentenceSpan> {
    if breakpoints.len() < 2 {
        return Vec::new();
    }

    let mut spans = Vec::with_capacity(breakpoints.len() - 1);

    for window in breakpoints.windows(2) {
        let mut start = window[0];
        let mut end = window[1];

        // Push start boundary if it falls inside atomic range
        start = push_boundary_to_safe(start, positions);

        // Push end boundary if it falls inside atomic range
        // But skip if start == end (empty span)
        if start < end {
            end = push_boundary_to_safe(end, positions);

            // After pushing both boundaries, check again
            if start < end && start <= end {
                spans.push(SentenceSpan {
                    char_start: start,
                    char_end: end,
                });
            }
        }
    }

    spans
}

/// Given a flat_text byte offset, find the TextPosition that contains it.
/// Used during projection to locate where §N markers should be inserted.
pub fn find_inline_position(
    positions: &[TextPosition],
    flat_offset: usize,
) -> Option<&TextPosition> {
    // First check exact boundaries: prefer the position ending at this offset
    // (left-leaning boundary) over one starting at it.
    positions
        .iter()
        .find(|p| p.flat_end == flat_offset)
        .or_else(|| {
            positions
                .iter()
                .find(|p| p.flat_start <= flat_offset && flat_offset < p.flat_end)
        })
}

// ── Private: Walker ─────────────────────────────────────────────────────────

fn walk_inlines(
    inlines: &[Inline],
    flat: &mut String,
    positions: &mut Vec<TextPosition>,
    container_stack: &mut Vec<(usize, usize, &'static str)>,
    inherited_atomic: bool,
) {
    for (idx, inline) in inlines.iter().enumerate() {
        match inline {
            // ── TYPE_A: Leaf text, CAN split ──────────────────────────
            Inline::Str(s) => {
                let start = flat.len();
                flat.push_str(s);
                let end = flat.len();
                push_position(
                    positions,
                    container_stack,
                    idx,
                    start,
                    end,
                    "Str",
                    inherited_atomic,
                );
            }

            Inline::Space => {
                let start = flat.len();
                flat.push(' ');
                let end = flat.len();
                push_position(
                    positions,
                    container_stack,
                    idx,
                    start,
                    end,
                    "Space",
                    inherited_atomic,
                );
            }

            Inline::SoftBreak => {
                // SoftBreak → ' ' (space, NOT newline — per spec)
                let start = flat.len();
                flat.push(' ');
                let end = flat.len();
                push_position(
                    positions,
                    container_stack,
                    idx,
                    start,
                    end,
                    "SoftBreak",
                    inherited_atomic,
                );
            }

            Inline::LineBreak => {
                // LineBreak → ' ' (space, NOT newline — per spec)
                let start = flat.len();
                flat.push(' ');
                let end = flat.len();
                push_position(
                    positions,
                    container_stack,
                    idx,
                    start,
                    end,
                    "LineBreak",
                    inherited_atomic,
                );
            }

            // ── TYPE_B: Atomic text, CANNOT split ─────────────────────
            Inline::Code(_, s) => {
                let start = flat.len();
                flat.push_str(s);
                let end = flat.len();
                push_position(positions, container_stack, idx, start, end, "Code", true);
            }

            Inline::Math(_, s) => {
                let start = flat.len();
                flat.push_str(s);
                let end = flat.len();
                push_position(positions, container_stack, idx, start, end, "Math", true);
            }

            Inline::RawInline(_, s) => {
                let start = flat.len();
                flat.push_str(s);
                let end = flat.len();
                push_position(
                    positions,
                    container_stack,
                    idx,
                    start,
                    end,
                    "RawInline",
                    true,
                );
            }

            // ── TYPE_C: Container, recurse ────────────────────────────
            Inline::Emph(children) => {
                recurse_into_container(
                    flat,
                    positions,
                    container_stack,
                    idx,
                    "Emph",
                    children,
                    inherited_atomic,
                );
            }

            Inline::Strong(children) => {
                recurse_into_container(
                    flat,
                    positions,
                    container_stack,
                    idx,
                    "Strong",
                    children,
                    inherited_atomic,
                );
            }

            Inline::Underline(children) => {
                recurse_into_container(
                    flat,
                    positions,
                    container_stack,
                    idx,
                    "Underline",
                    children,
                    inherited_atomic,
                );
            }

            Inline::Strikeout(children) => {
                recurse_into_container(
                    flat,
                    positions,
                    container_stack,
                    idx,
                    "Strikeout",
                    children,
                    inherited_atomic,
                );
            }

            Inline::Superscript(children) => {
                recurse_into_container(
                    flat,
                    positions,
                    container_stack,
                    idx,
                    "Superscript",
                    children,
                    inherited_atomic,
                );
            }

            Inline::Subscript(children) => {
                recurse_into_container(
                    flat,
                    positions,
                    container_stack,
                    idx,
                    "Subscript",
                    children,
                    inherited_atomic,
                );
            }

            Inline::SmallCaps(children) => {
                recurse_into_container(
                    flat,
                    positions,
                    container_stack,
                    idx,
                    "SmallCaps",
                    children,
                    inherited_atomic,
                );
            }

            Inline::Quoted(_quote_type, children) => {
                // Quoted: children only, NO quote characters in flat text
                recurse_into_container(
                    flat,
                    positions,
                    container_stack,
                    idx,
                    "Quoted",
                    children,
                    inherited_atomic,
                );
            }

            Inline::Span(_attr, children) => {
                recurse_into_container(
                    flat,
                    positions,
                    container_stack,
                    idx,
                    "Span",
                    children,
                    inherited_atomic,
                );
            }

            Inline::Link(_attr, children, _target) => {
                // Link: only link text (children), NOT the URL
                recurse_into_container(
                    flat,
                    positions,
                    container_stack,
                    idx,
                    "Link",
                    children,
                    inherited_atomic,
                );
            }

            Inline::Cite(_citations, children) => {
                // Cite: only citation text (children), not the citation metadata
                recurse_into_container(
                    flat,
                    positions,
                    container_stack,
                    idx,
                    "Cite",
                    children,
                    inherited_atomic,
                );
            }

            // ── TYPE_D: Skip entirely ─────────────────────────────────
            Inline::Note(_blocks) => {
                // Note → ZERO characters contributed. Skipped.
                // The position index does not advance.
            }

            // ── TYPE_E: Atomic caption ────────────────────────────────
            Inline::Image(_attr, caption, _target) => {
                // Image: caption text, marked atomic
                // Record the container entry at this point
                let container_start = flat.len();
                container_stack.push((idx, container_start, "Image"));
                // Mark children as atomic
                walk_inlines(caption, flat, positions, container_stack, true);
                container_stack.pop();
            }
        }
    }
}

/// Push a container onto the stack, recurse into children, pop.
fn recurse_into_container(
    flat: &mut String,
    positions: &mut Vec<TextPosition>,
    container_stack: &mut Vec<(usize, usize, &'static str)>,
    idx: usize,
    kind: &'static str,
    children: &[Inline],
    inherited_atomic: bool,
) {
    let container_start = flat.len();
    container_stack.push((idx, container_start, kind));
    walk_inlines(children, flat, positions, container_stack, inherited_atomic);
    container_stack.pop();
}

/// Create a TextPosition for a text run and push it to the positions vector.
fn push_position(
    positions: &mut Vec<TextPosition>,
    container_stack: &[(usize, usize, &'static str)],
    leaf_idx: usize,
    flat_start: usize,
    flat_end: usize,
    leaf_kind: &'static str,
    is_atomic: bool,
) {
    let mut inline_stack: Vec<TextOffsetInInline> = Vec::with_capacity(container_stack.len() + 1);

    for &(child_idx, container_start, kind) in container_stack.iter() {
        inline_stack.push(TextOffsetInInline {
            inline_index: child_idx,
            offset_within_inline: flat_start.saturating_sub(container_start),
            inline_kind: kind,
        });
    }

    // Add the leaf entry
    // For the leaf, compute the offset within this leaf's text contribution
    // by finding the container start of the parent
    let parent_start = container_stack.last().map(|&(_, s, _)| s).unwrap_or(0);
    inline_stack.push(TextOffsetInInline {
        inline_index: leaf_idx,
        offset_within_inline: flat_start.saturating_sub(parent_start),
        inline_kind: leaf_kind,
    });

    positions.push(TextPosition {
        flat_start,
        flat_end,
        inline_stack,
        is_atomic,
    });
}

// ── Private: Boundary Pushing ───────────────────────────────────────────────

/// Push a sentence boundary to the nearest safe position if it falls
/// inside an ATOMIC text range.
///
/// Rule:
///   distance_from_start = boundary - atomic_range.flat_start
///   distance_from_end = atomic_range.flat_end - boundary
///   if distance_from_start < distance_from_end:
///       boundary = atomic_range.flat_start (sentence ends before atomic block)
///   else:
///       boundary = atomic_range.flat_end   (sentence starts after atomic block)
fn push_boundary_to_safe(boundary: usize, positions: &[TextPosition]) -> usize {
    // Find the position that contains this boundary (non-inclusive of flat_end)
    let containing = positions
        .iter()
        .find(|p| p.flat_start <= boundary && boundary < p.flat_end);

    match containing {
        Some(pos) if pos.is_atomic => {
            let distance_from_start = boundary.saturating_sub(pos.flat_start);
            let distance_from_end = pos.flat_end.saturating_sub(boundary);

            if distance_from_start < distance_from_end {
                pos.flat_start
            } else {
                pos.flat_end
            }
        }
        _ => boundary, // Not in an atomic range, or boundary at exact edge — keep as-is
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Helper: build a quick paragraph ────────────────────────────────

    fn s(text: &str) -> Inline {
        Inline::Str(text.to_string())
    }

    fn space() -> Inline {
        Inline::Space
    }

    #[test]
    fn test_simple_two_sentence_paragraph() {
        // "Hello world. This is fine."
        let inlines = vec![
            s("Hello"),
            space(),
            s("world."),
            space(),
            s("This"),
            space(),
            s("is"),
            space(),
            s("fine."),
        ];
        let (flat, positions) = extract_text_with_positions(&inlines);
        assert_eq!(flat, "Hello world. This is fine.");

        // Verify flat text is correct
        assert_eq!(flat.len(), 26);

        // All positions should be non-atomic (TYPE_A only)
        for p in &positions {
            assert!(!p.is_atomic);
        }

        // Verify positions cover the entire flat text
        let mut covered = 0usize;
        for p in &positions {
            assert_eq!(covered, p.flat_start, "positions should be contiguous");
            covered = p.flat_end;
        }
        assert_eq!(covered, flat.len());
    }

    #[test]
    fn test_single_sentence() {
        let inlines = vec![s("Hello world.")];
        let (flat, positions) = extract_text_with_positions(&inlines);
        assert_eq!(flat, "Hello world.");
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0].flat_start, 0);
        assert_eq!(positions[0].flat_end, 12);
        assert_eq!(positions[0].inline_stack[0].inline_kind, "Str");
    }

    #[test]
    fn test_empty_paragraph() {
        let inlines: Vec<Inline> = vec![];
        let (flat, positions) = extract_text_with_positions(&inlines);
        assert_eq!(flat, "");
        assert!(positions.is_empty());
    }

    #[test]
    fn test_whitespace_only_paragraph() {
        let inlines = vec![Inline::Space, Inline::Space, Inline::SoftBreak];
        let (flat, positions) = extract_text_with_positions(&inlines);
        assert_eq!(flat, "   ");
        assert_eq!(positions.len(), 3);
    }

    #[test]
    fn test_strong_with_sentence_boundary_inside() {
        // Strong([Str("gravity wins. Always")]) → sentence boundary inside Strong
        let inlines = vec![Inline::Strong(vec![
            s("gravity"),
            space(),
            s("wins."),
            space(),
            s("Always"),
        ])];
        let (flat, positions) = extract_text_with_positions(&inlines);
        assert_eq!(flat, "gravity wins. Always");

        // All positions should be non-atomic (TYPE_C containers are not atomic)
        for p in &positions {
            assert!(!p.is_atomic);
        }

        // The inline_stack should have 2 levels: Strong → leaf
        for p in &positions {
            assert_eq!(p.inline_stack.len(), 2);
            assert_eq!(p.inline_stack[0].inline_kind, "Strong");
        }
    }

    #[test]
    fn test_code_block_atomic() {
        // Code is TYPE_B → atomic
        let inlines = vec![
            s("See "),
            Inline::Code(
                pandoc_ast::Attr::default(),
                "fn main() { return 1 }".to_string(),
            ),
            s(" That's it."),
        ];
        let (_flat, positions) = extract_text_with_positions(&inlines);

        // Code position should be atomic
        let code_pos = positions
            .iter()
            .find(|p| p.inline_stack.last().unwrap().inline_kind == "Code");
        assert!(code_pos.is_some());
        assert!(code_pos.unwrap().is_atomic);

        // Str positions should NOT be atomic
        let str_positions: Vec<_> = positions
            .iter()
            .filter(|p| p.inline_stack.last().unwrap().inline_kind == "Str")
            .collect();
        for p in &str_positions {
            assert!(!p.is_atomic);
        }
    }

    #[test]
    fn test_math_atomic() {
        let inlines = vec![
            s("E = "),
            Inline::Math(pandoc_ast::MathType::InlineMath, "mc^2".to_string()),
        ];
        let (flat, positions) = extract_text_with_positions(&inlines);
        assert_eq!(flat, "E = mc^2");

        let math_pos = positions
            .iter()
            .find(|p| p.inline_stack.last().unwrap().inline_kind == "Math");
        assert!(math_pos.is_some());
        assert!(math_pos.unwrap().is_atomic);
    }

    #[test]
    fn test_linebreak_treated_as_space() {
        let inlines = vec![s("Line"), Inline::LineBreak, s("broken")];
        let (flat, _positions) = extract_text_with_positions(&inlines);
        // LineBreak → ' ' (space, NOT newline)
        assert_eq!(flat, "Line broken");
        assert!(!flat.contains('\n'));
    }

    #[test]
    fn test_softbreak_treated_as_space() {
        let inlines = vec![s("Soft"), Inline::SoftBreak, s("break")];
        let (flat, _positions) = extract_text_with_positions(&inlines);
        assert_eq!(flat, "Soft break");
        assert!(!flat.contains('\n'));
        // SoftBreak should NOT be a sentence boundary (it's just a space)
    }

    #[test]
    fn test_quoted_no_quote_chars() {
        // Quoted(_, [Str("hello")]) → flat text should be "hello", not "\"hello\""
        let inlines = vec![
            s("He said "),
            Inline::Quoted(pandoc_ast::QuoteType::DoubleQuote, vec![s("hello")]),
            s(" to me."),
        ];
        let (flat, _positions) = extract_text_with_positions(&inlines);
        assert_eq!(flat, "He said hello to me.");
        // No quote characters should appear
        assert!(!flat.contains('"'));
        assert!(!flat.contains('\u{201c}')); // left double quote
        assert!(!flat.contains('\u{201d}')); // right double quote
    }

    #[test]
    fn test_link_only_text_no_url() {
        // Link(attr, [Str("click here")], target) → only "click here", not the URL
        let inlines = vec![
            s("Please "),
            Inline::Link(
                pandoc_ast::Attr::default(),
                vec![s("click here")],
                pandoc_ast::Target::default(),
            ),
            s(" for more."),
        ];
        let (flat, _positions) = extract_text_with_positions(&inlines);
        assert_eq!(flat, "Please click here for more.");
        // URL should not appear
        assert!(!flat.contains("http"));
    }

    #[test]
    fn test_cite_only_citation_text() {
        // Cite(citations, [Str("Doe 2020")]) → only "Doe 2020"
        let inlines = vec![
            s("According to "),
            Inline::Cite(vec![], vec![s("Doe 2020")]),
            s("."),
        ];
        let (flat, _positions) = extract_text_with_positions(&inlines);
        assert_eq!(flat, "According to Doe 2020.");
    }

    #[test]
    fn test_note_skipped_entirely() {
        // Note(blocks) → ZERO characters contributed
        let inlines = vec![s("Main text."), Inline::Note(vec![]), s(" More text.")];
        let (flat, positions) = extract_text_with_positions(&inlines);
        assert_eq!(flat, "Main text. More text.");
        // No Note position should exist
        let note_positions: Vec<_> = positions
            .iter()
            .filter(|p| p.inline_stack.iter().any(|e| e.inline_kind == "Note"))
            .collect();
        assert!(note_positions.is_empty());
    }

    #[test]
    fn test_image_caption_atomic() {
        // Image(attr, [Str("Figure 1")], target) → caption text, marked atomic
        let inlines = vec![
            s("See "),
            Inline::Image(
                pandoc_ast::Attr::default(),
                vec![s("Figure 1: A diagram")],
                pandoc_ast::Target::default(),
            ),
            s(" for details."),
        ];
        let (flat, positions) = extract_text_with_positions(&inlines);
        assert_eq!(flat, "See Figure 1: A diagram for details.");

        // Image caption positions should be atomic (inherited from Image container)
        let image_positions: Vec<_> = positions
            .iter()
            .filter(|p| p.inline_stack.iter().any(|e| e.inline_kind == "Image"))
            .collect();
        for p in &image_positions {
            assert!(p.is_atomic, "Image caption text should be atomic");
        }
    }

    #[test]
    fn test_nested_formatting_emph_in_strong() {
        // Strong([Emph([Str("bold italic")])])
        let inlines = vec![Inline::Strong(vec![Inline::Emph(vec![s("bold italic")])])];
        let (flat, positions) = extract_text_with_positions(&inlines);
        assert_eq!(flat, "bold italic");

        // inline_stack should have 3 levels: Strong → Emph → Str
        for p in &positions {
            assert_eq!(p.inline_stack.len(), 3);
            assert_eq!(p.inline_stack[0].inline_kind, "Strong");
            assert_eq!(p.inline_stack[1].inline_kind, "Emph");
            assert_eq!(p.inline_stack[2].inline_kind, "Str");
        }
    }

    #[test]
    fn test_mixed_atomic_and_type_a() {
        // Mix of TYPE_A and TYPE_B in sequence
        let inlines = vec![
            s("When "),
            Inline::Code(pandoc_ast::Attr::default(), "x = 5".to_string()),
            s(" the value is "),
            Inline::Math(pandoc_ast::MathType::InlineMath, "y^2".to_string()),
            s("."),
        ];
        let (flat, positions) = extract_text_with_positions(&inlines);
        assert_eq!(flat, "When x = 5 the value is y^2.");

        // Verify positions are contiguous
        let mut covered = 0usize;
        for p in &positions {
            assert_eq!(
                covered, p.flat_start,
                "positions should be contiguous at {}",
                covered
            );
            covered = p.flat_end;
        }
        assert_eq!(covered, flat.len());

        // Code and Math should be atomic
        let code_pos = positions
            .iter()
            .find(|p| p.inline_stack.last().unwrap().inline_kind == "Code");
        assert!(code_pos.unwrap().is_atomic);

        let math_pos = positions
            .iter()
            .find(|p| p.inline_stack.last().unwrap().inline_kind == "Math");
        assert!(math_pos.unwrap().is_atomic);
    }

    // ── map_sentence_boundaries tests ──────────────────────────────────

    #[test]
    fn test_map_sentence_boundaries_simple() {
        let inlines = vec![s("Hello world."), space(), s("This is fine.")];
        let (flat, positions) = extract_text_with_positions(&inlines);

        // Simulate icu_segmenter breakpoints
        let breakpoints = vec![0, 13, 29];
        let spans = map_sentence_boundaries(&flat, &positions, &breakpoints);

        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].char_start, 0);
        assert_eq!(spans[0].char_end, 13);
        assert_eq!(spans[1].char_start, 13);
        assert_eq!(spans[1].char_end, 29);
    }

    #[test]
    fn test_map_boundaries_atomic_push() {
        // "See fn main() { return 1 } That's it."
        // Total: 4 + 22 + 11 = 37 chars
        // Code "fn main() { return 1 }" = 22 chars, flat_start=4, flat_end=26
        let inlines = vec![
            s("See "),
            Inline::Code(
                pandoc_ast::Attr::default(),
                "fn main() { return 1 }".to_string(),
            ),
            s(" That's it."),
        ];
        let (flat, positions) = extract_text_with_positions(&inlines);

        // Simulate a breakpoint inside the Code block
        // Code spans [4..26). Breakpoint at 25 is inside atomic range.
        let breakpoints = vec![0, 25, 37];
        let spans = map_sentence_boundaries(&flat, &positions, &breakpoints);

        // Boundary at 25 falls inside Code [4..26):
        // distance_from_start = 25 - 4 = 21
        // distance_from_end = 26 - 25 = 1
        // 1 < 21 → push to flat_end → boundary = 26
        // So sentence 1 ends at 26, sentence 2 starts at 26
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].char_start, 0);
        assert_eq!(spans[0].char_end, 26); // pushed
        assert_eq!(spans[1].char_start, 26);
        assert_eq!(spans[1].char_end, 37);
    }

    #[test]
    fn test_find_inline_position() {
        let inlines = vec![s("Hello"), space(), s("world.")];
        let (_flat, positions) = extract_text_with_positions(&inlines);

        let pos = find_inline_position(&positions, 0);
        assert!(pos.is_some());
        assert_eq!(pos.unwrap().flat_start, 0);

        let pos = find_inline_position(&positions, 6);
        assert!(pos.is_some());
        assert_eq!(
            pos.unwrap().inline_stack.last().unwrap().inline_kind,
            "Space"
        );

        let pos = find_inline_position(&positions, 100);
        assert!(pos.is_none());
    }

    #[test]
    fn test_all_20_variants_covered() {
        // This test verifies that all 20 Inline variants are explicitly
        // handled in the walk_inlines match statement.
        // We do this by creating one of each variant and ensuring
        // extract_text_with_positions does not panic.

        let inlines = vec![
            // TYPE_A
            Inline::Str("text".to_string()),
            Inline::Space,
            Inline::SoftBreak,
            Inline::LineBreak,
            // TYPE_B
            Inline::Code(pandoc_ast::Attr::default(), "code".to_string()),
            Inline::Math(pandoc_ast::MathType::InlineMath, "math".to_string()),
            Inline::RawInline(pandoc_ast::Format("html".to_string()), "raw".to_string()),
            // TYPE_C
            Inline::Emph(vec![Inline::Str("emph".to_string())]),
            Inline::Strong(vec![Inline::Str("strong".to_string())]),
            Inline::Underline(vec![Inline::Str("underline".to_string())]),
            Inline::Strikeout(vec![Inline::Str("strike".to_string())]),
            Inline::Superscript(vec![Inline::Str("super".to_string())]),
            Inline::Subscript(vec![Inline::Str("sub".to_string())]),
            Inline::SmallCaps(vec![Inline::Str("caps".to_string())]),
            Inline::Quoted(
                pandoc_ast::QuoteType::DoubleQuote,
                vec![Inline::Str("quoted".to_string())],
            ),
            Inline::Span(
                pandoc_ast::Attr::default(),
                vec![Inline::Str("span".to_string())],
            ),
            Inline::Link(
                pandoc_ast::Attr::default(),
                vec![Inline::Str("link".to_string())],
                pandoc_ast::Target::default(),
            ),
            Inline::Cite(vec![], vec![Inline::Str("cite".to_string())]),
            // TYPE_D
            Inline::Note(vec![]),
            // TYPE_E
            Inline::Image(
                pandoc_ast::Attr::default(),
                vec![Inline::Str("caption".to_string())],
                pandoc_ast::Target::default(),
            ),
        ];

        let (flat, positions) = extract_text_with_positions(&inlines);

        // Flat text should have content from all non-skipped variants
        assert!(flat.contains("text"));
        assert!(flat.contains("code"));
        assert!(flat.contains("math"));
        assert!(flat.contains("raw"));
        assert!(flat.contains("emph"));
        assert!(flat.contains("strong"));
        assert!(flat.contains("underline"));
        assert!(flat.contains("strike"));
        assert!(flat.contains("super"));
        assert!(flat.contains("sub"));
        assert!(flat.contains("caps"));
        assert!(flat.contains("quoted"));
        assert!(flat.contains("span"));
        assert!(flat.contains("link"));
        assert!(flat.contains("cite"));
        // Note is skipped
        assert!(flat.contains("caption"));

        // Position count should match contributions (each leaf creates a position)
        assert!(!positions.is_empty());
    }
}
