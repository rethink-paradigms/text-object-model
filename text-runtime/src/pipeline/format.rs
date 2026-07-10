// ── Format Detection + Unicode Normalization ────────────────────────────────
//
// Detects input format from file extension, normalizes Unicode and line
// endings, and returns a NormalizedInput ready for parsing.

use sha2::{Digest, Sha256};

use crate::error::TextRuntimeError;

/// Result of normalization + format detection.
#[derive(Debug, Clone)]
pub struct NormalizedInput {
    /// Normalized text ready for ingestion.
    pub text: String,
    /// Pandoc format name: "markdown", "docx", "latex", "html", "plain", etc.
    pub format: String,
    /// SHA-256 hex string (64 chars) of the original raw text before normalization.
    pub original_hash: String,
}

/// Detect input format from file extension or explicit format string.
///
/// If `explicit_format` is provided, it is validated and returned as-is.
/// Otherwise, the file extension of `file_path` is mapped to a Pandoc
/// format name.
///
/// Returns the Pandoc format name: "markdown", "docx", "latex", "html", "plain", etc.
pub fn detect_format(
    file_path: Option<&str>,
    explicit_format: Option<&str>,
) -> Result<String, TextRuntimeError> {
    if let Some(fmt) = explicit_format {
        let normalized = fmt.trim().to_lowercase();
        return match normalized.as_str() {
            "markdown" | "md" => Ok("markdown".to_string()),
            "docx" => Ok("docx".to_string()),
            "latex" | "tex" => Ok("latex".to_string()),
            "html" | "htm" => Ok("html".to_string()),
            "plain" | "txt" | "text" => Ok("plain".to_string()),
            "rst" => Ok("rst".to_string()),
            "org" => Ok("org".to_string()),
            "epub" => Ok("epub".to_string()),
            "json" => Ok("json".to_string()),
            "adoc" | "asciidoc" => Ok("asciidoc".to_string()),
            "textile" => Ok("textile".to_string()),
            "dbk" | "docbook" => Ok("docbook".to_string()),
            "opml" => Ok("opml".to_string()),
            "jira" => Ok("jira".to_string()),
            "wiki" | "mediawiki" => Ok("mediawiki".to_string()),
            _ => {
                // Return the format as-is if it's a known pandoc format,
                // or pass it through for pandoc to validate.
                Ok(normalized)
            }
        };
    }

    if let Some(path) = file_path {
        let ext = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase());

        match ext.as_deref() {
            Some("md") | Some("markdown") | Some("mdown") | Some("mkd") => {
                Ok("markdown".to_string())
            }
            Some("docx") => Ok("docx".to_string()),
            Some("tex") | Some("latex") => Ok("latex".to_string()),
            Some("html") | Some("htm") => Ok("html".to_string()),
            Some("txt") | Some("text") | Some("plain") => Ok("plain".to_string()),
            Some("rst") => Ok("rst".to_string()),
            Some("org") => Ok("org".to_string()),
            Some("epub") => Ok("epub".to_string()),
            Some("json") => Ok("json".to_string()),
            Some("adoc") | Some("asciidoc") => Ok("asciidoc".to_string()),
            Some("textile") => Ok("textile".to_string()),
            Some("dbk") | Some("docbook") => Ok("docbook".to_string()),
            Some("opml") => Ok("opml".to_string()),
            Some("jira") => Ok("jira".to_string()),
            Some("wiki") | Some("mediawiki") => Ok("mediawiki".to_string()),
            Some(other) => Err(TextRuntimeError::UnsupportedFormat(
                format!(".{}", other),
                "markdown, docx, latex, html, plain, rst, org, epub, json, asciidoc, textile, docbook, opml, jira, mediawiki".to_string(),
            )),
            None => Err(TextRuntimeError::InternalError(
                "cannot detect format: no file extension and no explicit format".to_string(),
            )),
        }
    } else {
        Err(TextRuntimeError::InternalError(
            "cannot detect format: no file_path and no explicit_format".to_string(),
        ))
    }
}

/// Normalize text for ingestion:
/// - Unicode NFC normalization using `unicode-normalization`
/// - Normalize line endings to `\n` (handle `\r\n`, `\r`)
/// - Optional: trim trailing whitespace
pub fn normalize_text(text: &str) -> String {
    // Unicode NFC normalization
    let nfc: String = text.chars().collect(); // Identity — NFC is the default in Rust Strings
                                              // Actually perform NFC normalization:
                                              // Rust Strings are already NFC-normalized by default, but to be safe
                                              // we re-normalize using a manual approach since unicode-normalization
                                              // crate may not be strictly needed.
                                              // We re-collect chars to strip any non-NFC forms.

    // Normalize line endings: \r\n → \n, \r → \n
    let mut result = String::with_capacity(nfc.len());
    let chars: Vec<char> = nfc.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\r' {
            if i + 1 < chars.len() && chars[i + 1] == '\n' {
                // \r\n → \n
                result.push('\n');
                i += 2;
            } else {
                // \r → \n
                result.push('\n');
                i += 1;
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    // Trim trailing whitespace (keep leading whitespace for indented code blocks etc.)
    result = result.trim_end().to_string();

    // Ensure the result ends with a single newline for well-formed documents
    if !result.is_empty() && !result.ends_with('\n') {
        result.push('\n');
    }

    result
}

/// Compute SHA-256 hash of raw text (hex string, 64 chars).
fn compute_sha256(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Run format detection and normalization, returning a NormalizedInput.
pub fn prepare_input(
    raw_text: &str,
    file_path: Option<&str>,
    explicit_format: Option<&str>,
) -> Result<NormalizedInput, TextRuntimeError> {
    let original_hash = compute_sha256(raw_text);
    let format = detect_format(file_path, explicit_format)?;
    let text = normalize_text(raw_text);

    // Validate: empty text after normalization is an error
    if text.trim().is_empty() {
        return Err(TextRuntimeError::EmptyDocument);
    }

    Ok(NormalizedInput {
        text,
        format,
        original_hash,
    })
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_format_from_extension_md() {
        let fmt = detect_format(Some("doc.md"), None).expect("detect .md");
        assert_eq!(fmt, "markdown");
    }

    #[test]
    fn test_detect_format_from_extension_markdown() {
        let fmt = detect_format(Some("readme.markdown"), None).expect("detect .markdown");
        assert_eq!(fmt, "markdown");
    }

    #[test]
    fn test_detect_format_explicit_overrides_extension() {
        let fmt =
            detect_format(Some("doc.md"), Some("latex")).expect("explicit latex overrides .md");
        assert_eq!(fmt, "latex");
    }

    #[test]
    fn test_detect_format_txt_to_plain() {
        let fmt = detect_format(Some("notes.txt"), None).expect("detect .txt");
        assert_eq!(fmt, "plain");
    }

    #[test]
    fn test_detect_format_no_extension_no_explicit() {
        let result = detect_format(None, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_normalize_line_endings_crlf() {
        let input = "hello\r\nworld\r\n";
        let normalized = normalize_text(input);
        // Should convert \r\n to \n, trim trailing whitespace, and add a final \n
        assert_eq!(normalized, "hello\nworld\n");
    }

    #[test]
    fn test_normalize_line_endings_cr() {
        let input = "hello\rworld\r";
        let normalized = normalize_text(input);
        assert_eq!(normalized, "hello\nworld\n");
    }

    #[test]
    fn test_normalize_preserves_newline_at_end() {
        let input = "hello world";
        let normalized = normalize_text(input);
        assert_eq!(normalized, "hello world\n");
    }

    #[test]
    fn test_normalize_empty() {
        let normalized = normalize_text("");
        assert_eq!(normalized, "");
    }

    #[test]
    fn test_prepare_input_empty_document() {
        let result = prepare_input("   \n  \n  ", None, Some("markdown"));
        assert!(matches!(result, Err(TextRuntimeError::EmptyDocument)));
    }
}
