// ── Document Projection + §N Markers ───────────────────────────────────────
//
// The reverse of ingestion: reads a document's node tree from SQLite,
// assembles the Pandoc AST from content files, optionally injects §N
// sentence markers, and renders to the requested output format via
// pandoc-server.

use std::collections::HashMap;

use pandoc_ast::{Block, Pandoc};

use crate::cfg::RuntimeConfig;
use crate::error::TextRuntimeError;
use crate::store::content::ContentStore;
use crate::store::db::DbStore;
use crate::types::Uuid;

/// A rendered document projection.
///
/// `text` is the rendered output (markdown, html, etc.). `format` is the
/// output format name. `marker_map` is Some(session_map) when `markers`
/// was enabled — it maps §N numbers to sentence UUIDs.
#[derive(Debug, Clone)]
pub struct Projection {
    pub text: String,
    pub format: String,
    pub marker_map: Option<HashMap<u32, String>>,
}

/// Project a document to an output format, optionally with §N markers.
///
/// # Arguments
///
/// * `db` — The SQLite database store
/// * `content` — The content file store
/// * `config` — Runtime configuration (for pandoc port, locale, etc.)
/// * `doc_id` — The document UUID to project
/// * `format` — Target output format ("markdown", "html", "plain", etc.)
/// * `markers` — If true, inject §N markers at sentence boundaries
///
/// # Steps
///
/// 1. Load all nodes for the document from SQLite (ORDER BY position)
/// 2. For block nodes (has_content=1): read content file, deserialize Block
/// 3. For sentence nodes: read parent paragraph content, slice by offsets
/// 4. Assemble full Pandoc AST Vec<Block>
/// 5. If markers: inject §N markers and build marker_map
/// 6. Send assembled AST to pandoc-server for rendering
pub fn project_document(
    db: &DbStore,
    content: &ContentStore,
    _config: &RuntimeConfig,
    doc_id: &str,
    format: &str,
    markers: bool,
) -> Result<Projection, TextRuntimeError> {
    // 1. Load all nodes for this document
    let nodes = db.get_nodes_by_doc(doc_id)?;

    if nodes.is_empty() {
        return Err(TextRuntimeError::DocumentNotFound(doc_id.to_string()));
    }

    // 2. Find the root node (node_type = "document")
    let root_node = nodes
        .iter()
        .find(|n| n.node_type == "document")
        .ok_or_else(|| {
            TextRuntimeError::InternalError(format!("document '{}' has no root node", doc_id))
        })?;

    // 3. Build the Pandoc AST by walking the tree
    let blocks = build_block_list(content, &nodes, &root_node.uuid)?;

    // 4. Inject §N markers if requested
    let mut pandoc_blocks = blocks;
    let marker_map = if markers {
        let (marked_blocks, map) = inject_sentence_markers(pandoc_blocks)?;
        pandoc_blocks = marked_blocks;
        Some(map)
    } else {
        None
    };

    // 5. Assemble Pandoc AST
    let pandoc = Pandoc {
        meta: std::collections::BTreeMap::new(),
        blocks: pandoc_blocks,
        pandoc_api_version: vec![1, 23],
    };

    // 6. Serialize Pandoc AST to JSON
    let ast_json = serde_json::to_string(&pandoc)?;

    // 7. Render via pandoc-server
    // We use a simple synchronous HTTP call to pandoc-server
    // In a proper implementation this would be async, but for now
    // we render by running pandoc as a subprocess or using the server.
    //
    // For the initial implementation, we render locally:
    // serialize the AST and format it as pandoc expects.
    //
    // Since we can't easily call the pandoc-server synchronously,
    // and the PandocManager is async, we provide a best-effort
    // rendering. The text is the raw AST JSON if pandoc is unavailable.
    let rendered = render_ast_locally(&ast_json, &pandoc, format);

    Ok(Projection {
        text: rendered,
        format: format.to_string(),
        marker_map,
    })
}

// ── Block Tree Assembly ────────────────────────────────────────────────────

/// Build the list of blocks by walking the node tree in document order.
///
/// For each direct child of the given parent:
/// - If has_content: read the content file and deserialize the Block
/// - If it's a container (has children): wrap in appropriate block type
/// - If it's a sentence: skip (sentences are sub-block markers, not Pandoc blocks)
fn build_block_list(
    content: &ContentStore,
    all_nodes: &[crate::store::types::NodeRow],
    parent_uuid: &str,
) -> Result<Vec<Block>, TextRuntimeError> {
    // Get children of this parent from the pre-loaded node list
    let children: Vec<&crate::store::types::NodeRow> = all_nodes
        .iter()
        .filter(|n| n.parent_uuid.as_deref() == Some(parent_uuid))
        .collect();

    let mut blocks = Vec::new();

    for child in children {
        match child.node_type.as_str() {
            "paragraph" | "heading" | "code_block" | "blockquote" | "list_item" | "table"
            | "thematic_break" => {
                // These nodes store their Pandoc AST as a content file
                if child.has_content {
                    if let Some(ref content_path) = child.content_path {
                        // content_path is "xx/uuid.json" — extract uuid
                        let uuid_str = content_path
                            .rsplit('/')
                            .next()
                            .and_then(|s| s.strip_suffix(".json"))
                            .unwrap_or(&child.uuid);

                        let uuid: Uuid = uuid_str
                            .parse()
                            .map_err(|_| TextRuntimeError::InvalidUuid(uuid_str.to_string()))?;

                        let raw = content.get(&uuid)?;
                        let block: Block = serde_json::from_slice(&raw)?;
                        blocks.push(block);
                    }
                } else {
                    // Container node (e.g., BlockQuote, ListItem) — recurse
                    // For now, wrap in a generic paragraph with children's text
                    let inner_blocks = build_block_list(content, all_nodes, &child.uuid)?;
                    blocks.extend(inner_blocks);
                }
            }

            "section" | "document" => {
                // Recurse into container
                let inner_blocks = build_block_list(content, all_nodes, &child.uuid)?;
                blocks.extend(inner_blocks);
            }

            "sentence" => {
                // Sentences are sub-block annotations, not Pandoc blocks.
                // They are only used for §N marker injection.
                // Skip them during normal projection.
            }

            other => {
                // Unknown node type — skip with a log
                eprintln!(
                    "WARNING: unknown node type '{}' in projection, skipping",
                    other
                );
            }
        }
    }

    Ok(blocks)
}

// ── §N Marker Injection ────────────────────────────────────────────────────

/// Walk the Pandoc block list and inject §N markers at sentence boundaries.
///
/// Sentence boundaries are identified by sentence child nodes in the
/// node tree. This function:
/// 1. Finds all sentence nodes for the document
/// 2. Maps their char_start/char_end offsets into the paragraph's inline array
/// 3. Injects "§N " prefix at each sentence boundary
///
/// Returns (modified_blocks, marker_map) where marker_map maps §N → UUID.
fn inject_sentence_markers(
    _blocks: Vec<Block>,
) -> Result<(Vec<Block>, HashMap<u32, String>), TextRuntimeError> {
    // For now, return blocks unchanged with an empty marker map.
    // Full marker injection requires walking the inline array with
    // position index mapping, which is complex. This is a placeholder
    // for the complete implementation.
    //
    // TODO: Implement full §N marker injection using inline_mapping
    // and sentence node positions from the database.
    let map = HashMap::new();
    Ok((_blocks, map))
}

// ── Local Rendering ─────────────────────────────────────────────────────────

/// Render a Pandoc AST to the target format.
///
/// Currently returns the raw AST JSON as a fallback. In production, this
/// would call pandoc-server's HTTP API to convert from "json" to the
/// target format.
///
/// For markdown output, we provide a minimal markdown renderer that
/// handles the most common block types.
fn render_ast_locally(ast_json: &str, pandoc: &Pandoc, format: &str) -> String {
    // For now, render markdown locally for common block types.
    // In production, this would be handled by pandoc-server.
    if format == "markdown" || format == "md" {
        render_markdown(pandoc)
    } else if format == "plain" || format == "txt" {
        render_plain_text(pandoc)
    } else {
        // Fallback: return raw AST JSON
        ast_json.to_string()
    }
}

/// Minimal markdown renderer for common block types.
fn render_markdown(pandoc: &Pandoc) -> String {
    let mut output = String::new();
    for block in &pandoc.blocks {
        render_block_markdown(block, &mut output, 0);
    }
    output
}

/// Render a single block as markdown.
fn render_block_markdown(block: &Block, output: &mut String, _depth: usize) {
    match block {
        Block::Para(inlines) => {
            output.push_str(&render_inlines_markdown(inlines));
            output.push_str("\n\n");
        }
        Block::Plain(inlines) => {
            output.push_str(&render_inlines_markdown(inlines));
            output.push('\n');
        }
        Block::Header(level, _attr, inlines) => {
            let hashes = "#".repeat(*level as usize);
            output.push_str(&format!(
                "{} {}\n\n",
                hashes,
                render_inlines_markdown(inlines)
            ));
        }
        Block::CodeBlock(_attr, text) => {
            output.push_str(&format!("```\n{}\n```\n\n", text));
        }
        Block::BlockQuote(inner_blocks) => {
            for b in inner_blocks {
                let mut inner = String::new();
                render_block_markdown(b, &mut inner, 0);
                for line in inner.lines() {
                    output.push_str(&format!("> {}\n", line));
                }
            }
            output.push('\n');
        }
        Block::BulletList(items) => {
            for item_blocks in items {
                output.push_str("- ");
                let mut first = true;
                for b in item_blocks {
                    let mut inner = String::new();
                    render_block_markdown(b, &mut inner, 0);
                    let text = inner.trim();
                    if first {
                        output.push_str(text);
                        first = false;
                    } else {
                        output.push_str(&format!("  {}", text));
                    }
                }
                output.push('\n');
            }
            output.push('\n');
        }
        Block::OrderedList(_attrs, items) => {
            for (i, item_blocks) in items.iter().enumerate() {
                output.push_str(&format!("{}. ", i + 1));
                let mut first = true;
                for b in item_blocks {
                    let mut inner = String::new();
                    render_block_markdown(b, &mut inner, 0);
                    let text = inner.trim();
                    if first {
                        output.push_str(text);
                        first = false;
                    } else {
                        output.push_str(&format!("  {}", text));
                    }
                }
                output.push('\n');
            }
            output.push('\n');
        }
        Block::HorizontalRule => {
            output.push_str("---\n\n");
        }
        Block::Div(_attr, inner_blocks) => {
            for b in inner_blocks {
                render_block_markdown(b, output, 0);
            }
        }
        _ => {
            // Unknown block type — render as plain text
            let json = serde_json::to_string(block).unwrap_or_default();
            output.push_str(&json);
            output.push('\n');
        }
    }
}

/// Render inline elements as markdown.
fn render_inlines_markdown(inlines: &[pandoc_ast::Inline]) -> String {
    let mut result = String::new();
    for inline in inlines {
        match inline {
            pandoc_ast::Inline::Str(s) => result.push_str(s),
            pandoc_ast::Inline::Space => result.push(' '),
            pandoc_ast::Inline::SoftBreak => result.push(' '),
            pandoc_ast::Inline::LineBreak => result.push_str("\\\n"),
            pandoc_ast::Inline::Code(_attr, s) => {
                result.push('`');
                result.push_str(s);
                result.push('`');
            }
            pandoc_ast::Inline::Emph(children) => {
                result.push('*');
                result.push_str(&render_inlines_markdown(children));
                result.push('*');
            }
            pandoc_ast::Inline::Strong(children) => {
                result.push_str("**");
                result.push_str(&render_inlines_markdown(children));
                result.push_str("**");
            }
            pandoc_ast::Inline::Link(_attr, children, target) => {
                result.push('[');
                result.push_str(&render_inlines_markdown(children));
                result.push(']');
                result.push('(');
                result.push_str(&target.0);
                result.push(')');
            }
            pandoc_ast::Inline::Image(_attr, caption, target) => {
                result.push_str("![");
                result.push_str(&render_inlines_markdown(caption));
                result.push_str("](");
                result.push_str(&target.0);
                result.push(')');
            }
            pandoc_ast::Inline::Math(math_type, s) => match math_type {
                pandoc_ast::MathType::InlineMath => {
                    result.push('$');
                    result.push_str(s);
                    result.push('$');
                }
                pandoc_ast::MathType::DisplayMath => {
                    result.push_str("$$");
                    result.push_str(s);
                    result.push_str("$$");
                }
            },
            pandoc_ast::Inline::Strikeout(children) => {
                result.push_str("~~");
                result.push_str(&render_inlines_markdown(children));
                result.push_str("~~");
            }
            pandoc_ast::Inline::Quoted(quote_type, children) => {
                let (open, close) = match quote_type {
                    pandoc_ast::QuoteType::SingleQuote => ("'", "'"),
                    pandoc_ast::QuoteType::DoubleQuote => ("\"", "\""),
                };
                result.push_str(open);
                result.push_str(&render_inlines_markdown(children));
                result.push_str(close);
            }
            pandoc_ast::Inline::Cite(_citations, children) => {
                result.push_str(&render_inlines_markdown(children));
            }
            pandoc_ast::Inline::Note(_blocks) => {
                // Skip footnotes in markdown rendering
            }
            _ => {
                // Other inline types: skip or render minimally
                if let Ok(json) = serde_json::to_string(inline) {
                    result.push_str(&json);
                }
            }
        }
    }
    result
}

/// Render blocks as plain text (just extract text content).
fn render_plain_text(pandoc: &Pandoc) -> String {
    let mut output = String::new();
    for block in &pandoc.blocks {
        render_block_plain(block, &mut output);
    }
    output
}

fn render_block_plain(block: &Block, output: &mut String) {
    match block {
        Block::Para(inlines) | Block::Plain(inlines) => {
            output.push_str(&render_inlines_markdown(inlines));
            output.push_str("\n\n");
        }
        Block::Header(_level, _attr, inlines) => {
            output.push_str(&render_inlines_markdown(inlines));
            output.push_str("\n\n");
        }
        Block::CodeBlock(_attr, text) => {
            output.push_str(text);
            output.push_str("\n\n");
        }
        Block::BlockQuote(inner_blocks) => {
            for b in inner_blocks {
                render_block_plain(b, output);
            }
        }
        Block::BulletList(items) => {
            for item_blocks in items {
                output.push_str("- ");
                for b in item_blocks {
                    render_block_plain(b, output);
                }
            }
            output.push('\n');
        }
        Block::OrderedList(_attrs, items) => {
            for (i, item_blocks) in items.iter().enumerate() {
                output.push_str(&format!("{}. ", i + 1));
                for b in item_blocks {
                    render_block_plain(b, output);
                }
            }
            output.push('\n');
        }
        Block::HorizontalRule => {
            output.push_str("---\n\n");
        }
        Block::Div(_attr, inner_blocks) => {
            for b in inner_blocks {
                render_block_plain(b, output);
            }
        }
        _ => {}
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_markdown_simple_paragraph() {
        let pandoc = Pandoc {
            meta: std::collections::BTreeMap::new(),
            blocks: vec![Block::Para(vec![pandoc_ast::Inline::Str(
                "Hello world.".to_string(),
            )])],
            pandoc_api_version: vec![1, 23],
        };

        let md = render_markdown(&pandoc);
        assert_eq!(md, "Hello world.\n\n");
    }

    #[test]
    fn test_render_markdown_heading() {
        let pandoc = Pandoc {
            meta: std::collections::BTreeMap::new(),
            blocks: vec![
                Block::Header(
                    2,
                    pandoc_ast::Attr::default(),
                    vec![pandoc_ast::Inline::Str("Introduction".to_string())],
                ),
                Block::Para(vec![pandoc_ast::Inline::Str("Some text.".to_string())]),
            ],
            pandoc_api_version: vec![1, 23],
        };

        let md = render_markdown(&pandoc);
        assert!(md.starts_with("## Introduction"));
        assert!(md.contains("Some text."));
    }

    #[test]
    fn test_render_markdown_empty() {
        let pandoc = Pandoc {
            meta: std::collections::BTreeMap::new(),
            blocks: vec![],
            pandoc_api_version: vec![1, 23],
        };

        let md = render_markdown(&pandoc);
        assert_eq!(md, "");
    }

    #[test]
    fn test_render_inlines_emphasis() {
        let inlines = vec![
            pandoc_ast::Inline::Str("Hello ".to_string()),
            pandoc_ast::Inline::Emph(vec![pandoc_ast::Inline::Str("world".to_string())]),
            pandoc_ast::Inline::Str(".".to_string()),
        ];

        let rendered = render_inlines_markdown(&inlines);
        assert_eq!(rendered, "Hello *world*.");
    }

    #[test]
    fn test_render_inlines_code() {
        let inlines = vec![
            pandoc_ast::Inline::Str("Use ".to_string()),
            pandoc_ast::Inline::Code(pandoc_ast::Attr::default(), "cargo build".to_string()),
            pandoc_ast::Inline::Str(" to compile.".to_string()),
        ];

        let rendered = render_inlines_markdown(&inlines);
        assert_eq!(rendered, "Use `cargo build` to compile.");
    }
}
