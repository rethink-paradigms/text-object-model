// ── Structural Segmenter ────────────────────────────────────────────────────
//
// Walks the Pandoc AST and produces a tree of StructuralNodes:
// document → sections → paragraphs/headings/code blocks/etc → sentences.
//
// Uses icu_segmenter for sentence boundary detection on paragraph inlines,
// then maps those boundaries through inline_mapping to create sentence
// child nodes with char_start/char_end offsets.

use icu_segmenter::SentenceSegmenter;
use pandoc_ast::{Block, Inline, Pandoc};

use crate::error::TextRuntimeError;
use crate::pipeline::inline_mapping;
use crate::types::{NodeType, SentenceSpan, StructuralNode};

/// Walk the Pandoc AST and produce a tree of StructuralNodes.
///
/// Hierarchy: document → sections → paragraphs/headings/code_blocks/etc → sentences.
///
/// Each block node gets position (gap-based float: 1000, 2000, 3000...).
/// Each paragraph's inline array is processed through inline_mapping +
/// icu_segmenter to identify sentence boundaries.
pub fn segment(pandoc: &Pandoc) -> Result<StructuralNode, TextRuntimeError> {
    let mut root = StructuralNode {
        uuid: None,
        node_type: NodeType::Document,
        parent_uuid: None,
        position: 0.0,
        plain_text: String::new(),
        structural_hash: String::new(),
        has_content: false,
        char_start: None,
        char_end: None,
        heading_level: None,
        section_path: None,
        version: 1,
        children: Vec::new(),
        pandoc_ast_json: None,
    };

    let mut position_counter: f64 = 1000.0;
    walk_blocks(&pandoc.blocks, &mut root, &mut position_counter)?;

    Ok(root)
}

// ── Block walker ────────────────────────────────────────────────────────────

fn walk_blocks(
    blocks: &[Block],
    parent: &mut StructuralNode,
    position_counter: &mut f64,
) -> Result<(), TextRuntimeError> {
    for block in blocks {
        match block {
            // ── Paragraph: segment into sentences ─────────────────────
            Block::Para(inlines) | Block::Plain(inlines) => {
                let pos = *position_counter;
                *position_counter += 1000.0;

                let (flat_text, _positions) = inline_mapping::extract_text_with_positions(inlines);

                // Build paragraph node
                let mut para_node = StructuralNode {
                    uuid: None,
                    node_type: NodeType::Paragraph,
                    parent_uuid: parent.uuid,
                    position: pos,
                    plain_text: flat_text.clone(),
                    structural_hash: String::new(), // filled by uuid_assigner
                    has_content: true,
                    char_start: None,
                    char_end: None,
                    heading_level: None,
                    section_path: None,
                    version: 1,
                    children: Vec::new(),
                    pandoc_ast_json: Some(block_to_json(block)),
                };

                // Segment into sentences using icu_segmenter
                let sentences = segment_paragraph(inlines, &flat_text)?;

                // Add sentence children
                for sentence in sentences {
                    let sent_node = StructuralNode {
                        uuid: None,
                        node_type: NodeType::Sentence,
                        parent_uuid: None, // will be set after UUID assignment
                        position: *position_counter,
                        plain_text: flat_text[sentence.char_start..sentence.char_end].to_string(),
                        structural_hash: String::new(), // filled by uuid_assigner
                        has_content: false,
                        char_start: Some(sentence.char_start),
                        char_end: Some(sentence.char_end),
                        heading_level: None,
                        section_path: None,
                        version: 1,
                        children: Vec::new(),
                        pandoc_ast_json: None,
                    };
                    *position_counter += 1000.0;
                    para_node.children.push(sent_node);
                }

                // Add paragraph to parent
                parent.children.push(para_node);
            }

            // ── Heading ──────────────────────────────────────────────
            Block::Header(level, _attr, inlines) => {
                let pos = *position_counter;
                *position_counter += 1000.0;

                let (flat_text, _positions) = inline_mapping::extract_text_with_positions(inlines);

                let heading_node = StructuralNode {
                    uuid: None,
                    node_type: NodeType::Heading,
                    parent_uuid: parent.uuid,
                    position: pos,
                    plain_text: flat_text,
                    structural_hash: String::new(),
                    has_content: true,
                    char_start: None,
                    char_end: None,
                    heading_level: Some(*level as i32),
                    section_path: None, // filled later if needed
                    version: 1,
                    children: Vec::new(),
                    pandoc_ast_json: Some(block_to_json(block)),
                };

                parent.children.push(heading_node);
            }

            // ── Code Block ────────────────────────────────────────────
            Block::CodeBlock(_attr, text) => {
                let pos = *position_counter;
                *position_counter += 1000.0;

                let node = StructuralNode {
                    uuid: None,
                    node_type: NodeType::CodeBlock,
                    parent_uuid: parent.uuid,
                    position: pos,
                    plain_text: text.clone(),
                    structural_hash: String::new(),
                    has_content: true,
                    char_start: None,
                    char_end: None,
                    heading_level: None,
                    section_path: None,
                    version: 1,
                    children: Vec::new(),
                    pandoc_ast_json: Some(block_to_json(block)),
                };

                parent.children.push(node);
            }

            // ── BlockQuote ────────────────────────────────────────────
            Block::BlockQuote(inner_blocks) => {
                let pos = *position_counter;
                *position_counter += 1000.0;

                let mut bq_node = StructuralNode {
                    uuid: None,
                    node_type: NodeType::BlockQuote,
                    parent_uuid: parent.uuid,
                    position: pos,
                    plain_text: String::new(),
                    structural_hash: String::new(),
                    has_content: true,
                    char_start: None,
                    char_end: None,
                    heading_level: None,
                    section_path: None,
                    version: 1,
                    children: Vec::new(),
                    pandoc_ast_json: Some(block_to_json(block)),
                };

                walk_blocks(inner_blocks, &mut bq_node, position_counter)?;

                // Accumulate plain_text from children
                bq_node.plain_text = bq_node
                    .children
                    .iter()
                    .map(|c| c.plain_text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");

                parent.children.push(bq_node);
            }

            // ── Ordered List ──────────────────────────────────────────
            Block::OrderedList(_list_attrs, items) => {
                let pos = *position_counter;
                *position_counter += 1000.0;

                let mut list_node = StructuralNode {
                    uuid: None,
                    node_type: NodeType::ListItem, // list itself is not a separate type; items are
                    parent_uuid: parent.uuid,
                    position: pos,
                    plain_text: String::new(),
                    structural_hash: String::new(),
                    has_content: true,
                    char_start: None,
                    char_end: None,
                    heading_level: None,
                    section_path: None,
                    version: 1,
                    children: Vec::new(),
                    pandoc_ast_json: Some(block_to_json(block)),
                };

                for item_blocks in items {
                    let item_pos = *position_counter;
                    *position_counter += 1000.0;

                    let mut item_node = StructuralNode {
                        uuid: None,
                        node_type: NodeType::ListItem,
                        parent_uuid: None,
                        position: item_pos,
                        plain_text: String::new(),
                        structural_hash: String::new(),
                        has_content: false,
                        char_start: None,
                        char_end: None,
                        heading_level: None,
                        section_path: None,
                        version: 1,
                        children: Vec::new(),
                        pandoc_ast_json: None,
                    };

                    walk_blocks(item_blocks, &mut item_node, position_counter)?;

                    // Accumulate plain_text from children
                    item_node.plain_text = item_node
                        .children
                        .iter()
                        .map(|c| c.plain_text.as_str())
                        .collect::<Vec<_>>()
                        .join(" ");

                    list_node.children.push(item_node);
                }

                parent.children.push(list_node);
            }

            // ── Bullet List ───────────────────────────────────────────
            Block::BulletList(items) => {
                let pos = *position_counter;
                *position_counter += 1000.0;

                let mut list_node = StructuralNode {
                    uuid: None,
                    node_type: NodeType::ListItem,
                    parent_uuid: parent.uuid,
                    position: pos,
                    plain_text: String::new(),
                    structural_hash: String::new(),
                    has_content: true,
                    char_start: None,
                    char_end: None,
                    heading_level: None,
                    section_path: None,
                    version: 1,
                    children: Vec::new(),
                    pandoc_ast_json: Some(block_to_json(block)),
                };

                for item_blocks in items {
                    let item_pos = *position_counter;
                    *position_counter += 1000.0;

                    let mut item_node = StructuralNode {
                        uuid: None,
                        node_type: NodeType::ListItem,
                        parent_uuid: None,
                        position: item_pos,
                        plain_text: String::new(),
                        structural_hash: String::new(),
                        has_content: false,
                        char_start: None,
                        char_end: None,
                        heading_level: None,
                        section_path: None,
                        version: 1,
                        children: Vec::new(),
                        pandoc_ast_json: None,
                    };

                    walk_blocks(item_blocks, &mut item_node, position_counter)?;

                    item_node.plain_text = item_node
                        .children
                        .iter()
                        .map(|c| c.plain_text.as_str())
                        .collect::<Vec<_>>()
                        .join(" ");

                    list_node.children.push(item_node);
                }

                parent.children.push(list_node);
            }

            // ── Table ─────────────────────────────────────────────────
            Block::Table(_attr, _caption, _colspecs, _thead, _tbodies, _tfoot) => {
                let pos = *position_counter;
                *position_counter += 1000.0;

                let table_node = StructuralNode {
                    uuid: None,
                    node_type: NodeType::Table,
                    parent_uuid: parent.uuid,
                    position: pos,
                    plain_text: String::new(),
                    structural_hash: String::new(),
                    has_content: true,
                    char_start: None,
                    char_end: None,
                    heading_level: None,
                    section_path: None,
                    version: 1,
                    children: Vec::new(),
                    pandoc_ast_json: Some(block_to_json(block)),
                };

                parent.children.push(table_node);
            }

            // ── HorizontalRule → ThematicBreak ────────────────────────
            Block::HorizontalRule => {
                let pos = *position_counter;
                *position_counter += 1000.0;

                let hr_node = StructuralNode {
                    uuid: None,
                    node_type: NodeType::ThematicBreak,
                    parent_uuid: parent.uuid,
                    position: pos,
                    plain_text: String::new(),
                    structural_hash: String::new(),
                    has_content: true,
                    char_start: None,
                    char_end: None,
                    heading_level: None,
                    section_path: None,
                    version: 1,
                    children: Vec::new(),
                    pandoc_ast_json: Some(block_to_json(block)),
                };

                parent.children.push(hr_node);
            }

            // ── Div → Section ─────────────────────────────────────────
            Block::Div(_attr, inner_blocks) => {
                let pos = *position_counter;
                *position_counter += 1000.0;

                let mut section_node = StructuralNode {
                    uuid: None,
                    node_type: NodeType::Section,
                    parent_uuid: parent.uuid,
                    position: pos,
                    plain_text: String::new(),
                    structural_hash: String::new(),
                    has_content: false,
                    char_start: None,
                    char_end: None,
                    heading_level: None,
                    section_path: None,
                    version: 1,
                    children: Vec::new(),
                    pandoc_ast_json: None,
                };

                walk_blocks(inner_blocks, &mut section_node, position_counter)?;

                // Accumulate plain_text from children
                section_node.plain_text = section_node
                    .children
                    .iter()
                    .map(|c| c.plain_text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");

                parent.children.push(section_node);
            }

            // ── Other block types: skip or handle minimally ───────────
            Block::LineBlock(_lines) => {
                // Treat as paragraph-like
                let pos = *position_counter;
                *position_counter += 1000.0;

                let mut flat_parts = Vec::new();
                for line in _lines {
                    let (flat, _) = inline_mapping::extract_text_with_positions(line);
                    flat_parts.push(flat);
                }
                let plain_text = flat_parts.join("\n");

                let node = StructuralNode {
                    uuid: None,
                    node_type: NodeType::Paragraph,
                    parent_uuid: parent.uuid,
                    position: pos,
                    plain_text,
                    structural_hash: String::new(),
                    has_content: true,
                    char_start: None,
                    char_end: None,
                    heading_level: None,
                    section_path: None,
                    version: 1,
                    children: Vec::new(),
                    pandoc_ast_json: Some(block_to_json(block)),
                };

                parent.children.push(node);
            }

            Block::RawBlock(_format, text) => {
                let pos = *position_counter;
                *position_counter += 1000.0;

                let node = StructuralNode {
                    uuid: None,
                    node_type: NodeType::Paragraph,
                    parent_uuid: parent.uuid,
                    position: pos,
                    plain_text: text.clone(),
                    structural_hash: String::new(),
                    has_content: true,
                    char_start: None,
                    char_end: None,
                    heading_level: None,
                    section_path: None,
                    version: 1,
                    children: Vec::new(),
                    pandoc_ast_json: Some(block_to_json(block)),
                };

                parent.children.push(node);
            }

            Block::DefinitionList(_items) => {
                // Skip for now — definition lists are complex
                let pos = *position_counter;
                *position_counter += 1000.0;

                let mut flat_parts = Vec::new();
                for (terms, _defs) in _items {
                    for term in terms {
                        let (flat, _) =
                            inline_mapping::extract_text_with_positions(std::slice::from_ref(term));
                        flat_parts.push(flat);
                    }
                }

                let node = StructuralNode {
                    uuid: None,
                    node_type: NodeType::Paragraph,
                    parent_uuid: parent.uuid,
                    position: pos,
                    plain_text: flat_parts.join(" "),
                    structural_hash: String::new(),
                    has_content: true,
                    char_start: None,
                    char_end: None,
                    heading_level: None,
                    section_path: None,
                    version: 1,
                    children: Vec::new(),
                    pandoc_ast_json: Some(block_to_json(block)),
                };

                parent.children.push(node);
            }

            // ── Figure ──────────────────────────────────────────────
            Block::Figure(_attr, _caption, inner_blocks) => {
                let pos = *position_counter;
                *position_counter += 1000.0;

                let mut figure_node = StructuralNode {
                    uuid: None,
                    node_type: NodeType::Section,
                    parent_uuid: parent.uuid,
                    position: pos,
                    plain_text: String::new(),
                    structural_hash: String::new(),
                    has_content: true,
                    char_start: None,
                    char_end: None,
                    heading_level: None,
                    section_path: None,
                    version: 1,
                    children: Vec::new(),
                    pandoc_ast_json: Some(block_to_json(block)),
                };

                walk_blocks(inner_blocks, &mut figure_node, position_counter)?;
                figure_node.plain_text = figure_node
                    .children
                    .iter()
                    .map(|c| c.plain_text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");

                parent.children.push(figure_node);
            }

            Block::Null => {
                // Skip Null blocks — they contribute nothing
            }
        }
    }

    Ok(())
}

// ── Sentence segmentation ───────────────────────────────────────────────────

/// Segment a paragraph's inline array into sentence spans.
///
/// Uses icu_segmenter for boundary detection and inline_mapping's
/// map_sentence_boundaries for atomic boundary push.
fn segment_paragraph(
    inlines: &[Inline],
    flat_text: &str,
) -> Result<Vec<SentenceSpan>, TextRuntimeError> {
    if flat_text.trim().is_empty() {
        return Ok(Vec::new());
    }

    // 1. Build position index
    let (_flat, positions) = inline_mapping::extract_text_with_positions(inlines);

    // 2. Run icu_segmenter
    let segmenter = SentenceSegmenter::new(Default::default());
    let breakpoints: Vec<usize> = segmenter.segment_str(flat_text).collect();

    // 3. Map breakpoints to sentence spans with atomic boundary push
    let spans = inline_mapping::map_sentence_boundaries(flat_text, &positions, &breakpoints);

    // 4. Filter out empty spans
    let non_empty: Vec<SentenceSpan> = spans
        .into_iter()
        .filter(|s| s.char_start < s.char_end)
        .collect();

    Ok(non_empty)
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Convert a Pandoc Block to a serde_json::Value for content file storage.
fn block_to_json(block: &Block) -> serde_json::Value {
    serde_json::to_value(block).unwrap_or(serde_json::Value::Null)
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pandoc(blocks: Vec<Block>) -> Pandoc {
        Pandoc {
            meta: std::collections::BTreeMap::new(),
            blocks,
            pandoc_api_version: vec![1, 23],
        }
    }

    #[test]
    fn test_segment_simple_paragraph() {
        let blocks = vec![Block::Para(vec![Inline::Str(
            "Hello world. This is fine.".to_string(),
        )])];
        let pandoc = make_pandoc(blocks);
        let root = segment(&pandoc).expect("segment");

        // Should have one paragraph child
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].node_type, NodeType::Paragraph);

        // Paragraph should have sentence children
        let para = &root.children[0];
        assert!(
            !para.children.is_empty(),
            "paragraph should have sentence children"
        );

        // All sentence children should be Sentence type
        for child in &para.children {
            assert_eq!(child.node_type, NodeType::Sentence);
        }
    }

    #[test]
    fn test_segment_heading() {
        let blocks = vec![Block::Header(
            2,
            pandoc_ast::Attr::default(),
            vec![Inline::Str("Methods".to_string())],
        )];
        let pandoc = make_pandoc(blocks);
        let root = segment(&pandoc).expect("segment");

        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].node_type, NodeType::Heading);
        assert_eq!(root.children[0].heading_level, Some(2));
        assert_eq!(root.children[0].plain_text, "Methods");
    }

    #[test]
    fn test_segment_code_block() {
        let blocks = vec![Block::CodeBlock(
            pandoc_ast::Attr::default(),
            "fn main() {\n    println!(\"hello\");\n}\n".to_string(),
        )];
        let pandoc = make_pandoc(blocks);
        let root = segment(&pandoc).expect("segment");

        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].node_type, NodeType::CodeBlock);
        assert!(root.children[0].plain_text.contains("fn main"));
    }

    #[test]
    fn test_segment_empty_document() {
        let pandoc = make_pandoc(vec![]);
        let root = segment(&pandoc).expect("segment");
        assert_eq!(root.node_type, NodeType::Document);
        assert!(root.children.is_empty());
    }

    #[test]
    fn test_segment_section_nesting() {
        // Div containing a heading and paragraph
        let blocks = vec![Block::Div(
            pandoc_ast::Attr::default(),
            vec![
                Block::Header(
                    1,
                    pandoc_ast::Attr::default(),
                    vec![Inline::Str("Introduction".to_string())],
                ),
                Block::Para(vec![Inline::Str("This is the intro.".to_string())]),
            ],
        )];
        let pandoc = make_pandoc(blocks);
        let root = segment(&pandoc).expect("segment");

        // Should have one section child
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].node_type, NodeType::Section);

        // Section should have heading and paragraph children
        let section = &root.children[0];
        assert_eq!(section.children.len(), 2);
        assert_eq!(section.children[0].node_type, NodeType::Heading);
        assert_eq!(section.children[1].node_type, NodeType::Paragraph);
    }

    #[test]
    fn test_segment_blockquote() {
        let blocks = vec![Block::BlockQuote(vec![Block::Para(vec![Inline::Str(
            "Quoted text.".to_string(),
        )])])];
        let pandoc = make_pandoc(blocks);
        let root = segment(&pandoc).expect("segment");

        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].node_type, NodeType::BlockQuote);
    }

    #[test]
    fn test_segment_horizontal_rule() {
        let blocks = vec![Block::HorizontalRule];
        let pandoc = make_pandoc(blocks);
        let root = segment(&pandoc).expect("segment");

        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].node_type, NodeType::ThematicBreak);
    }
}
