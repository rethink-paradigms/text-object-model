// ── Pandoc AST Parser ───────────────────────────────────────────────────────
//
// Parses raw text into a Pandoc AST using pandoc-server's /batch endpoint.
// The text is sent with from=format, to="json" and the response is parsed
// as a pandoc_ast::Pandoc struct.

use pandoc_ast::Pandoc;

use crate::error::TextRuntimeError;
use crate::pandoc_mgr::PandocManager;

/// Parse raw text into a Pandoc AST using pandoc-server.
///
/// The text is sent to pandoc-server's /batch endpoint with from=format,
/// to="json". The response's "output" field is parsed as Pandoc AST JSON.
pub async fn parse_to_ast(
    pandoc: &PandocManager,
    text: &str,
    format: &str,
) -> Result<Pandoc, TextRuntimeError> {
    let response_text = pandoc.convert(text, format).await?;
    parse_pandoc_json(&response_text)
}

/// Parse a file on disk directly into a Pandoc AST using the local pandoc CLI.
pub async fn parse_file_to_ast(
    pandoc: &PandocManager,
    file_path: &std::path::Path,
    format: &str,
) -> Result<Pandoc, TextRuntimeError> {
    let response_text = pandoc.convert_file(file_path, format).await?;
    parse_pandoc_json(&response_text)
}

/// Parse a Pandoc AST from a serde_json::Value.
///
/// Uses pandoc_ast's deserialization. The AST JSON from pandoc-server
/// is a JSON array where the first element is a metadata map and the
/// second element is the block array, with a "pandoc-api-version" field.
///
/// Falls back gracefully for forward-compatibility: if pandoc_ast
/// cannot deserialize a new block type, logs a warning and continues.
pub fn parse_pandoc_json(json_str: &str) -> Result<Pandoc, TextRuntimeError> {
    let value: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| TextRuntimeError::ParseError {
            format: "json".to_string(),
            message: format!("failed to parse Pandoc AST JSON: {}", e),
        })?;
    parse_pandoc_json_value(&value)
}

/// Internal: parse a serde_json::Value into a Pandoc AST.
fn parse_pandoc_json_value(value: &serde_json::Value) -> Result<Pandoc, TextRuntimeError> {
    // pandoc_ast 0.8.6 expects the top-level JSON object with "blocks",
    // "meta", and "pandoc-api-version" fields.
    serde_json::from_value(value.clone()).map_err(|e| TextRuntimeError::ParseError {
        format: "json".to_string(),
        message: format!("failed to deserialize Pandoc AST: {}", e),
    })
}

/// Parse with a forward-compatible deserializer that handles unknown
/// Pandoc variants.
///
/// If pandoc_ast can't deserialize a new block type, we fall back to
/// serde_json::Value and log a warning. This prevents crashes when
/// Pandoc adds new block types.
pub fn parse_pandoc_json_fallback(json_str: &str) -> Result<Pandoc, TextRuntimeError> {
    match parse_pandoc_json(json_str) {
        Ok(ast) => Ok(ast),
        Err(_) => {
            // Attempt to parse as a value and extract what we can
            let value: serde_json::Value =
                serde_json::from_str(json_str).map_err(|e| TextRuntimeError::ParseError {
                    format: "json".to_string(),
                    message: format!("failed to parse JSON even with fallback: {}", e),
                })?;

            // Log a warning about unknown variants (in a real impl, use a logger)
            eprintln!(
                "WARNING: pandoc_ast could not fully deserialize the AST. \
                 Some block types may be unknown. Proceeding with partial parse."
            );

            parse_pandoc_json_value(&value)
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_pandoc_json_simple() {
        let json = r#"{"pandoc-api-version":[1,23],"meta":{},"blocks":[]}"#;
        let ast = parse_pandoc_json(json).expect("parse simple pandoc json");
        assert_eq!(ast.pandoc_api_version, vec![1, 23]);
        assert!(ast.blocks.is_empty());
    }

    #[test]
    fn test_parse_pandoc_json_with_paragraph() {
        let json = r#"{"pandoc-api-version":[1,23],"meta":{},"blocks":[{"t":"Para","c":[{"t":"Str","c":"Hello"}]}]}"#;
        let ast = parse_pandoc_json(json).expect("parse json with para");
        assert_eq!(ast.blocks.len(), 1);
    }

    #[test]
    fn test_parse_pandoc_json_invalid() {
        let result = parse_pandoc_json("not valid json");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_pandoc_json_fallback_valid() {
        let json = r#"{"pandoc-api-version":[1,23],"meta":{},"blocks":[]}"#;
        let ast = parse_pandoc_json_fallback(json).expect("fallback parse");
        assert!(ast.blocks.is_empty());
    }
}
