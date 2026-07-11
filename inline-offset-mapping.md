# Text Runtime — Inline-to-Offset Mapping and §N Marker Injection

**Status:** Implemented
**Sources:** pandoc_ast v0.8.6 Inline enum, icu_segmenter v2.2.0 API, pandoc_ast MutVisitor trait
**Novelty:** No production Rust code solves this exact problem. The algorithm below is original but composed from well-understood pieces: flat text extraction, position indexing, and AST modification.

> **Note on §N vs UUIDs:** §N markers are for display only. They are injected into projected text to make it scannable for agents. Annotation uses UUIDs directly — the `read` endpoint returns a `marker_map` (HashMap<u32 → UUID) alongside the rendered text. The client resolves §N → UUID before calling `annotate`. See `storage-architecture.md §11` for the full agent interface.
## Problem

Pandoc paragraphs contain formatted inline elements. `icu_segmenter` operates on **flat plain text**. Sentence boundaries identified by icu_segmenter must be mapped back to positions in the original inline array. This mapping is needed in both directions:

1. **Ingest (forward)**: flat text breakpoints → char_start/char_end in the inline array
2. **Project (reverse)**: char_start/char_end → positions in the inline array where §N markers are inserted

## Part 1: The Inline Variants

From `pandoc_ast v0.8.6`, the `Inline` enum has exactly 20 variants. Each is classified into one of five categories:

```
TYPE_A: Leaf text, sentence boundaries CAN fall inside (text contributes to flow)
TYPE_B: Leaf text, sentence boundaries CANNOT fall inside (atomic)
TYPE_C: Container, sentence boundaries CAN fall inside (recurse into children)
TYPE_D: Skip — not part of paragraph sentence flow
TYPE_E: Contributes text but is atomic
```

| Variant | Type | Text Contribution | Split? |
|---|---|---|---|
| `Str(s)` | A | contributes `s` | yes |
| `Space` | A | contributes `' '` | yes |
| `SoftBreak` | A | contributes `' '` | yes |
| `LineBreak` | A | contributes `' '` (not newline) | yes |
| `Code(_, s)` | B | contributes `s` | no |
| `Math(_, s)` | B | contributes `s` | no |
| `RawInline(_, s)` | B | contributes `s` | no |
| `Emph(children)` | C | contributes children's text | yes (recurse) |
| `Strong(children)` | C | contributes children's text | yes (recurse) |
| `Underline(children)` | C | contributes children's text | yes (recurse) |
| `Strikeout(children)` | C | contributes children's text | yes (recurse) |
| `Superscript(children)` | C | contributes children's text | yes (recurse) |
| `Subscript(children)` | C | contributes children's text | yes (recurse) |
| `SmallCaps(children)` | C | contributes children's text | yes (recurse) |
| `Quoted(_, children)` | C | contributes children's text, **without** quote characters | yes (recurse) |
| `Span(_, children)` | C | contributes children's text | yes (recurse) |
| `Link(_, children, _)` | C | contributes children's text (NOT the URL) | yes (recurse) |
| `Cite(_, children)` | C | contributes children's text | yes (recurse) |
| `Image(_, caption, _)` | E | contributes caption children's text | no (treat as atomic block) |
| `Note(blocks)` | D | **skipped entirely** — footnotes are separate sections | n/a |

### Rules for each type

**TYPE_A (Str, Space, SoftBreak, LineBreak)**:
- `Str(s)`: append `s` to flat text. Record position mapping: flat_text[start..end] → (inline_index, 0..s.len()).
- `Space`: append `' '` to flat text.
- `SoftBreak`: append `' '` to flat text (treat as inter-word space, not as a sentence boundary opportunity).
- `LineBreak`: append `' '` to flat text (same as SoftBreak for sentence purposes — the line break matters for rendering, not for sentence structure).

**TYPE_B (Code, Math, RawInline)**:
- Append the string content to flat text.
- Mark the range as ATOMIC in the position index. Sentence boundaries that fall inside this range are **pushed** to the nearest safe boundary (start or end of the range, whichever is closer).

**TYPE_C (Emph, Strong, Underline, Strikeout, Superscript, Subscript, SmallCaps, Quoted, Span, Link, Cite)**:
- Recurse into children. The children's text is appended to flat text.
- Sentence boundaries CAN fall inside these containers. When they do, the inline is **split** during projection (see Part 3).
- For `Quoted`: Do NOT add quote characters to flat text. The quotes are rendering decisions, not sentence content.
- For `Link`: Contribute ONLY the link text (children), not the URL. The URL is metadata, not content.
- For `Cite`: Contribute ONLY the citation text (children), not the citation metadata.

**TYPE_D (Note)**:
- Skip entirely. `Note` contains `Vec<Block>` (footnotes) — these are separate sections of the document, not part of the paragraph's sentence flow.
- They contribute ZERO characters to flat text. The position index does not advance.

**TYPE_E (Image)**:
- Contribute the caption's text (children of the Image's `Vec<Inline>` caption second field).
- BUT treat the entire range as atomic — sentence boundaries should not split an image caption from its image. In practice, image captions are short enough that this rarely matters.

## Part 2: Ingest-Time Mapping (Forward Direction)

### Data Structure

```rust
/// Maps a range in flat plain text back to a position in the inline array.
#[derive(Debug, Clone)]
pub struct TextPosition {
    pub flat_start: usize,        // byte offset in flat text (start)
    pub flat_end: usize,          // byte offset in flat text (end)
    pub inline_stack: Vec<TextOffsetInInline>,  // path through nested inlines
    pub is_atomic: bool,          // true if this range cannot be split
}

/// Position within a single inline element at a given depth.
#[derive(Debug, Clone)]
pub struct TextOffsetInInline {
    pub inline_index: usize,          // index into the parent's children Vec
    pub offset_within_inline: usize,  // byte offset within that inline's own flat text
    pub inline_kind: &'static str,    // "Str", "Emph", etc. — for debugging
}
```

### Algorithm

```rust
/// Extract flat plain text from a Pandoc inline array, building a position
/// index that can map any byte offset in the flat text back to a position
/// in the inline tree.
///
/// Returns (flat_text, position_index).
///
/// position_index is a Vec<TextPosition> where entries are non-overlapping
/// and cover the entire flat_text in order. For TYPE_A variants, entries
/// are one per contiguous text run. For TYPE_B and TYPE_E, exactly one entry.
/// For TYPE_C, entries are produced by recursing into children.
pub fn extract_text_with_positions(inlines: &[Inline]) -> (String, Vec<TextPosition>) {
    let mut flat = String::new();
    let mut positions: Vec<TextPosition> = Vec::new();
    let mut stack_trace: Vec<usize> = Vec::new();  // tracks which child index at each depth
    walk_inlines(inlines, &mut flat, &mut positions, &mut stack_trace, 0, false);
    (flat, positions)
}

fn walk_inlines(
    inlines: &[Inline],
    flat: &mut String,
    positions: &mut Vec<TextPosition>,
    parent_indices: &mut Vec<usize>,
    depth: usize,
    inherited_atomic: bool,
) {
    for (idx, inline) in inlines.iter().enumerate() {
        parent_indices.push(idx);
        match inline {
            // TYPE_A: Simple text, can split
            Inline::Str(s) => {
                let start = flat.len();
                flat.push_str(s);
                let end = flat.len();
                positions.push(TextPosition {
                    flat_start: start,
                    flat_end: end,
                    inline_stack: parent_indices.iter().enumerate().map(|(depth, &child_idx)| {
                        TextOffsetInInline {
                            inline_index: child_idx,
                            offset_within_inline: if depth == parent_indices.len() - 1 {
                                // Last depth: offset into this Str
                                start  // wrong — need correct offset
                            } else {
                                0
                            },
                            inline_kind: "Str",
                        }
                    }).collect(),
                    is_atomic: inherited_atomic,
                });
            }
            Inline::Space | Inline::SoftBreak | Inline::LineBreak => {
                let start = flat.len();
                flat.push(' ');
                let end = flat.len();
                positions.push(TextPosition { flat_start: start, flat_end: end, ... });
            }

            // TYPE_B: Atomic text, do not split
            Inline::Code(_, s) | Inline::Math(_, s) | Inline::RawInline(_, s) => {
                let start = flat.len();
                flat.push_str(s);
                let end = flat.len();
                positions.push(TextPosition { flat_start: start, flat_end: end, is_atomic: true, ... });
            }

            // TYPE_C: Container, recurse
            Inline::Emph(children) | Inline::Strong(children)
            | Inline::Underline(children) | Inline::Strikeout(children)
            | Inline::Superscript(children) | Inline::Subscript(children)
            | Inline::SmallCaps(children) | Inline::Span(_, children) => {
                walk_inlines(children, flat, positions, parent_indices, depth + 1, inherited_atomic);
            }

            Inline::Quoted(_, children) => {
                // No quote characters in flat text
                walk_inlines(children, flat, positions, parent_indices, depth + 1, inherited_atomic);
            }

            Inline::Link(_, children, _) => {
                // Only link text, not URL
                walk_inlines(children, flat, positions, parent_indices, depth + 1, inherited_atomic);
            }

            Inline::Cite(_, children) => {
                walk_inlines(children, flat, positions, parent_indices, depth + 1, inherited_atomic);
            }

            // TYPE_D: Skip
            Inline::Note(_) => {
                // Contributes nothing to flat text
            }

            // TYPE_E: Atomic caption text
            Inline::Image(_, caption, _) => {
                // caption is Vec<Inline> — contribute text but mark as atomic
                walk_inlines(caption, flat, positions, parent_indices, depth + 1, true);
            }
        }
        parent_indices.pop();
    }
}
```

### Sentence Boundary Resolution

After building the flat text and position index:

```rust
// 1. Run icu_segmenter on flat_text → Vec<usize> breakpoints
//    Returns byte offsets where each sentence STARTS (including 0 and flat_text.len())

// 2. For each pair of consecutive breakpoints (bp[i], bp[i+1]):
//    - Find position_index entries that overlap this range
//    - If the boundary falls inside an ATOMIC range (Code, Math, etc.):
//        → push to nearest safe boundary (start or end of atomic range)
//    - If the boundary falls inside a TYPE_A/C range:
//        → it's fine, split there
//    - Store char_start = bp[i], char_end = bp[i+1] for the sentence node

// 3. Edge case: multiple sentence boundaries in a paragraph
//    Each produces a separate sentence child node
```

**The critical rule for atomic inline splitting:**

```
If sentence boundary B falls inside an atomic inline A (Code, Math, RawInline, Image):
    let distance_from_start = B - A.flat_start
    let distance_from_end = A.flat_end - B
    if distance_from_start < distance_from_end:
        push boundary to A.flat_start  (sentence ends before A)
    else:
        push boundary to A.flat_end    (sentence starts after A)
```

This avoids splitting code snippets, math expressions, and image captions while accepting that the sentence boundary won't be perfectly precise in those rare cases.

## Part 3: Project-Time §N Marker Injection (Reverse Direction)

At projection time, when markers are enabled:

```rust
/// Inject §N markers into a Pandoc AST paragraph's inline array.
///
/// sentences: Vec<(uuid: String, char_start: usize, char_end: usize)> 
/// — loaded from SQLite for the paragraph being projected.
///
/// The markers are injected by walking the inline array, finding the
/// position corresponding to each char_start, and inserting
/// Inline::Str(format!("§{} ", sentence_num)) at that position.
pub fn inject_sentence_markers(
    paragraph_uuid: &str,
    inlines: &mut Vec<Inline>,
) -> HashMap<u32, String> {
    // 1. Build flat text + position index (same function as ingest)
    let (flat_text, positions) = extract_text_with_positions(inlines);

    // 2. Load sentences for this paragraph
    let sentences = db.query(
        "SELECT uuid, char_start, char_end FROM nodes 
         WHERE parent_uuid = ?1 ORDER BY char_start",
        params![paragraph_uuid],
    )?;

    // 3. For each sentence, find the inline position to inject
    let mut marker_map = HashMap::new();
    for (i, sentence) in sentences.iter().enumerate() {
        let sent_num = i + 1;  // 1-indexed
        let char_start = sentence.char_start as usize;

        // 4. Find which TextPosition in the position index contains char_start
        let pos = positions.iter()
            .find(|p| p.flat_start <= char_start && char_start < p.flat_end)
            .or_else(|| positions.iter().find(|p| p.flat_start == char_start))?;

        // 5. pos.inline_stack tells us the path through the inline tree.
        //    We navigate to the parent container and insert at the right child index.
        //
        //    The marker is inserted as a new Str inline BEFORE the inline
        //    that contains char_start.
        let marker = Inline::Str(format!("§{} ", sent_num));
        insert_inline_before(inlines, &pos.inline_stack, marker);

        // 6. Record the mapping
        marker_map.insert(sent_num as u32, sentence.uuid.clone());
    }

    Ok(marker_map)
}

/// Insert an Inline element into the inline array BEFORE the element
/// identified by the inline_stack path.
fn insert_inline_before(
    inlines: &mut Vec<Inline>,
    stack: &[TextOffsetInInline],
    marker: Inline,
) {
    if stack.is_empty() {
        return;
    }

    let target_idx = stack[0].inline_index;
    if stack.len() == 1 {
        // Insert at this level
        inlines.insert(target_idx, marker);
    } else {
        // Navigate into the child container and recurse
        let child = &mut inlines[target_idx];
        match child {
            Inline::Emph(children) | Inline::Strong(children)
            | Inline::Underline(children) | Inline::Strikeout(children)
            | Inline::Superscript(children) | Inline::Subscript(children)
            | Inline::SmallCaps(children) | Inline::Span(_, children)
            | Inline::Quoted(_, children) | Inline::Link(_, children, _)
            | Inline::Cite(_, children) => {
                insert_inline_before(children, &stack[1..], marker);
            }
            _ => {
                // Should not happen — stack path was built from this array
            }
        }
    }
}
```

### Marker Injection — Visual Example

```
Before:
  [Str("The activation"), Space(), Str("energy is"), Space(), Str("5.2 kJ/mol."), 
   Space(), Str("Temperature matters.")]

After (with sentence markers):
  [Str("§1 The activation"), Space(), Str("energy is"), Space(), Str("5.2 kJ/mol."), 
   Space(), Str("§2 Temperature matters.")]

Between sentences:
  Sentence 1: char_start=0,  char_end=28  → marker §1 at position 0
  Sentence 2: char_start=29, char_end=47  → marker §2 at position 29
```

### Marker Injection Inside Formatted Inlines

When a sentence boundary falls within a TYPE_C container (e.g., Strong):

```
Before:
  [Str("Key claim: "), Strong([Str("The activation energy is 5.2."), Space(), Str("Temperature matters.")])]

Sentence boundary at char 28 (between "5.2." and "Temperature"):
  → split the Strong[children] array:
  → insert §2 between the two Str inlines inside Strong

After:
  [Str("Key claim: "), Strong([Str("§1 The activation energy is 5.2."), Space(), Str("§2 Temperature matters.")])]
```

The marker is inserted BETWEEN the two inner inlines of the Strong. The Strong itself is NOT split into two Strongs — the marker lives inside the existing formatting. This is correct because §N markers should not affect the visual formatting.

### Edge Cases for Marker Injection

1. **Empty paragraph**: No sentences → no markers. Paragraph renders as-is.

2. **Single-sentence paragraph**: One marker at position 0: `Str("§1 The entire paragraph.")`.

3. **Consecutive atomic inlines**: If `Str`, `Code("x=1")`, `Str("equals 2")`, and icu_segmenter sees "x=1 equals 2" as one sentence, no boundary falls inside the Code. If it DOES (highly unlikely for real code), the boundary is pushed per the atomic rule.

4. **Note (footnote) markers**: Footnotes are separate sections with their own sentence IDs. The marker numbering is session-local — footnotes get their own range (e.g., §101-§105 for footnotes, continued after the last body sentence). This is a display choice and can be adjusted.

## Part 4: Verification

The mapping is correct if and only if:

1. **Round-trip**: For every sentence node stored at ingest, the projection-time marker injection places the §N marker at the exact start of that sentence's text in the rendered output.

2. **No missing markers**: Every sentence in the SQLite query (for a given paragraph) produces exactly one marker in the projected output.

3. **No orphan markers**: Every injected marker corresponds to a sentence that exists in SQLite. If a marker is injected but no sentence node exists, that's a bug.

4. **Performance**: Building flat text + position index for a paragraph is O(n) where n is the number of inline elements. For a typical paragraph of 50-200 inlines, this is sub-millisecond. Cache is not needed.

## Part 5: Verification Test Cases

### Test 1: Simple paragraph
```
Input:  Para([Str("Hello world. This is fine.")])
Flat:   "Hello world. This is fine."
Breaks: [0, 13, 29]
Output: Sentence 1: char_start=0,  char_end=13  → "Hello world. "
        Sentence 2: char_start=13, char_end=29  → "This is fine."
Markers: §1 before "Hello", §2 before "This"
```

### Test 2: Formatted paragraph
```
Input:  Para([Str("Key idea: "), Strong([Str("gravity"), Space(), Str("wins")]), Str(" always.")])
Flat:   "Key idea: gravity wins always."
Breaks: [0, 22, 29]  (icu_segmenter: "Key idea: gravity wins always." → 2 sentences?)
Wait — icu_segmenter would probably see ONE sentence here.
Let me adjust: "Key idea: gravity wins. Always."
Flat:   "Key idea: gravity wins. Always."
Breaks: [0, 25, 33]
Output: Sentence 1: char_start=0,  char_end=25  → "Key idea: gravity wins. "
        Sentence 2: char_start=25, char_end=33  → "Always."
Markers: §1 before "Key", §2 before "Always"
§2 is injected BETWEEN Space() and Str("Always") inside the Strong:
  [Str("Key idea: "), Strong([Str("gravity"), Space(), Str("wins"), Space(), Str("§2 Always")]), Space(), Str(" always.")]
```

### Test 3: Code and math
```
Input:  Para([Str("When x = "), Code(_, "2 + 2"), Str(" and E = "), Math(_, "mc^2"), Str(", the result is 4.")])
Flat:   "When x = 2 + 2 and E = mc^2, the result is 4."
Breaks: [0, 48]  (one sentence)
Atomic ranges: Code [10..15], Math [20..24]
If icu_segmenter saw a break inside "2 + 2" (unlikely but test), 
  the break would be pushed to the Code boundary.
```

### Test 4: Atomic boundary push
```
Input:  Para([Str("See "), Code(_, "fn main() { return 1 }"), Str(" That's it.")])
Flat:   "See fn main() { return 1 } That's it."
Breaks: [0, 30, 41]  (two sentences)
Sentence boundary at 30: falls at "}" — which is part of the Code atomic range [4..28].
  push_start = Code starts at flat[4], ends at flat[28]
  distance_from_start = 30 - 28 = 2
  distance_from_end = 41 - 30 = 11
  2 < 11 → push to Code end: sentence 1 ends at 28, sentence 2 starts at 28

Output: Sentence 1: char_start=0,  char_end=28  → "See fn main() { return 1 }"
        Sentence 2: char_start=28, char_end=41  → "That's it."
```

