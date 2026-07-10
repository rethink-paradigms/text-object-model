// ── W3C Web Annotation Types ───────────────────────────────────────────────
//
// serde types for W3C JSON-LD Web Annotations, conforming to:
//   https://www.w3.org/TR/annotation-model/
//
// Supports:
//   - TextPositionSelector (character offset)
//   - TextQuoteSelector (exact text match)
//   - Compound selectors (position + quote for dual anchoring)
//   - TextualBody with motivation/purpose

use serde::{Deserialize, Serialize};

/// A position-based text selector.
///
/// Describes a span by its start and end character offsets within
/// the source document's plain text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextPositionSelector {
    #[serde(rename = "type")]
    pub type_: String,

    /// Start character offset (byte offset in UTF-8 text).
    pub start: usize,

    /// End character offset (byte offset in UTF-8 text).
    pub end: usize,
}

impl TextPositionSelector {
    /// Create a new TextPositionSelector.
    pub fn new(start: usize, end: usize) -> Self {
        Self {
            type_: "TextPositionSelector".to_string(),
            start,
            end,
        }
    }

    /// The length of the span in bytes.
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    /// Check if the span is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A quote-based text selector.
///
/// Describes a span by providing the exact text to match, plus optional
/// prefix and suffix for disambiguation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextQuoteSelector {
    #[serde(rename = "type")]
    pub type_: String,

    /// The exact text to match.
    pub exact: String,

    /// Optional prefix context (preceding text, up to 64 bytes).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,

    /// Optional suffix context (following text, up to 64 bytes).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suffix: Option<String>,
}

impl TextQuoteSelector {
    /// Create a new TextQuoteSelector with exact text only.
    pub fn new(exact: &str) -> Self {
        Self {
            type_: "TextQuoteSelector".to_string(),
            exact: exact.to_string(),
            prefix: None,
            suffix: None,
        }
    }

    /// Create a TextQuoteSelector with prefix and suffix context.
    pub fn with_context(exact: &str, prefix: &str, suffix: &str) -> Self {
        // Limit prefix/suffix to 64 bytes for W3C compliance
        let prefix = if prefix.len() > 64 {
            &prefix[prefix.len() - 64..]
        } else {
            prefix
        };
        let suffix = if suffix.len() > 64 {
            &suffix[..64]
        } else {
            suffix
        };

        Self {
            type_: "TextQuoteSelector".to_string(),
            exact: exact.to_string(),
            prefix: Some(prefix.to_string()),
            suffix: Some(suffix.to_string()),
        }
    }
}

/// A compound selector — can be either position-based or quote-based.
/// Uses serde's untagged enum for transparent JSON serialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Selector {
    /// Position-based selection by character offset.
    TextPosition(TextPositionSelector),

    /// Quote-based selection by exact text match.
    TextQuote(TextQuoteSelector),
}

impl Selector {
    /// Create a position selector.
    pub fn position(start: usize, end: usize) -> Self {
        Selector::TextPosition(TextPositionSelector::new(start, end))
    }

    /// Create a quote selector.
    pub fn quote(exact: &str) -> Self {
        Selector::TextQuote(TextQuoteSelector::new(exact))
    }

    /// Create a quote selector with context.
    pub fn quote_with_context(exact: &str, prefix: &str, suffix: &str) -> Self {
        Selector::TextQuote(TextQuoteSelector::with_context(exact, prefix, suffix))
    }

    /// Check if this is a position selector.
    pub fn is_position(&self) -> bool {
        matches!(self, Selector::TextPosition(_))
    }

    /// Check if this is a quote selector.
    pub fn is_quote(&self) -> bool {
        matches!(self, Selector::TextQuote(_))
    }

    /// Get the position range if this is a position selector.
    pub fn position_range(&self) -> Option<(usize, usize)> {
        match self {
            Selector::TextPosition(p) => Some((p.start, p.end)),
            _ => None,
        }
    }

    /// Get the exact text if this is a quote selector.
    pub fn exact_text(&self) -> Option<&str> {
        match self {
            Selector::TextQuote(q) => Some(&q.exact),
            _ => None,
        }
    }
}

/// The target of an annotation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnnotationTarget {
    /// The source document UUID or URL.
    pub source: String,

    /// Optional selectors for pinpointing the exact span.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<Vec<Selector>>,
}

/// A textual body of an annotation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextualBody {
    #[serde(rename = "type")]
    pub type_: String,

    /// The text content of the body.
    pub value: String,

    /// Optional purpose ("commenting", "describing", "tagging", etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,

    /// Optional format ("text/markdown", "text/plain", etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

impl TextualBody {
    /// Create a new textual body with a value.
    pub fn new(value: &str) -> Self {
        Self {
            type_: "TextualBody".to_string(),
            value: value.to_string(),
            purpose: None,
            format: None,
        }
    }

    /// Create a body with purpose and format.
    pub fn with_purpose(value: &str, purpose: &str, format: Option<&str>) -> Self {
        Self {
            type_: "TextualBody".to_string(),
            value: value.to_string(),
            purpose: Some(purpose.to_string()),
            format: format.map(|s| s.to_string()),
        }
    }
}

/// The body of an annotation — either a TextualBody or an arbitrary JSON value.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AnnotationBody {
    /// A structured textual body.
    TextualBody(TextualBody),

    /// An arbitrary JSON value (for extensibility).
    Value(serde_json::Value),
}

/// A complete W3C Web Annotation (JSON-LD).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Annotation {
    /// JSON-LD context.
    #[serde(rename = "@context")]
    pub context: String,

    /// Always "Annotation".
    #[serde(rename = "type")]
    pub type_: String,

    /// Unique annotation identifier (UUID).
    pub id: String,

    /// The target of the annotation.
    pub target: AnnotationTarget,

    /// The body (content) of the annotation.
    pub body: AnnotationBody,

    /// Motivation ("commenting", "describing", "tagging", "bookmarking", etc.).
    pub motivation: String,

    /// Creator identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creator: Option<String>,

    /// Creation timestamp (ISO 8601).
    pub created: String,

    /// Last modification timestamp (ISO 8601).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
}

impl Annotation {
    /// Create a new annotation with a position selector.
    // Position/quote fields are a cohesive set; a builder struct is tracked as
    // future debt. (clippy::too_many_arguments)
    #[allow(clippy::too_many_arguments)]
    pub fn with_position(
        id: &str,
        source: &str,
        start: usize,
        end: usize,
        body_value: &str,
        motivation: &str,
        creator: Option<&str>,
        created: &str,
    ) -> Self {
        Self {
            context: "http://www.w3.org/ns/anno.jsonld".to_string(),
            type_: "Annotation".to_string(),
            id: id.to_string(),
            target: AnnotationTarget {
                source: source.to_string(),
                selector: Some(vec![Selector::position(start, end)]),
            },
            body: AnnotationBody::TextualBody(TextualBody::new(body_value)),
            motivation: motivation.to_string(),
            creator: creator.map(|s| s.to_string()),
            created: created.to_string(),
            modified: Some(created.to_string()),
        }
    }

    /// Create a new annotation with both position and quote selectors.
    // Position/quote fields are a cohesive set; a builder struct is tracked as
    // future debt. (clippy::too_many_arguments)
    #[allow(clippy::too_many_arguments)]
    pub fn with_dual_selectors(
        id: &str,
        source: &str,
        start: usize,
        end: usize,
        exact: &str,
        prefix: Option<&str>,
        suffix: Option<&str>,
        body_value: &str,
        motivation: &str,
        creator: Option<&str>,
        created: &str,
    ) -> Self {
        let mut selectors = vec![Selector::position(start, end)];

        let quote_sel = if let (Some(p), Some(s)) = (prefix, suffix) {
            Selector::quote_with_context(exact, p, s)
        } else {
            Selector::quote(exact)
        };
        selectors.push(quote_sel);

        Self {
            context: "http://www.w3.org/ns/anno.jsonld".to_string(),
            type_: "Annotation".to_string(),
            id: id.to_string(),
            target: AnnotationTarget {
                source: source.to_string(),
                selector: Some(selectors),
            },
            body: AnnotationBody::TextualBody(TextualBody::new(body_value)),
            motivation: motivation.to_string(),
            creator: creator.map(|s| s.to_string()),
            created: created.to_string(),
            modified: Some(created.to_string()),
        }
    }

    /// Serialize the annotation to a JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Deserialize an annotation from a JSON string.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_position_selector() {
        let sel = TextPositionSelector::new(0, 42);
        assert_eq!(sel.type_, "TextPositionSelector");
        assert_eq!(sel.start, 0);
        assert_eq!(sel.end, 42);
        assert_eq!(sel.len(), 42);
        assert!(!sel.is_empty());
    }

    #[test]
    fn test_text_position_selector_empty() {
        let sel = TextPositionSelector::new(5, 5);
        assert!(sel.is_empty());
        assert_eq!(sel.len(), 0);
    }

    #[test]
    fn test_text_quote_selector() {
        let sel = TextQuoteSelector::new("hello world");
        assert_eq!(sel.type_, "TextQuoteSelector");
        assert_eq!(sel.exact, "hello world");
        assert!(sel.prefix.is_none());
        assert!(sel.suffix.is_none());
    }

    #[test]
    fn test_text_quote_selector_with_context() {
        let sel = TextQuoteSelector::with_context(
            "middle",
            "this is the beginning prefix text",
            "this is the ending suffix text that follows",
        );
        assert_eq!(sel.exact, "middle");
        // Prefix is limited to 64 bytes from the end
        assert!(sel.prefix.as_ref().unwrap().len() <= 64);
        // Suffix is limited to 64 bytes from the start
        assert!(sel.suffix.as_ref().unwrap().len() <= 64);
    }

    #[test]
    fn test_selector_enum() {
        let pos = Selector::position(10, 20);
        assert!(pos.is_position());
        assert!(!pos.is_quote());
        assert_eq!(pos.position_range(), Some((10, 20)));

        let quote = Selector::quote("test");
        assert!(quote.is_quote());
        assert!(!quote.is_position());
        assert_eq!(quote.exact_text(), Some("test"));
    }

    #[test]
    fn test_annotation_serialization() {
        let anno = Annotation::with_position(
            "uuid-123",
            "doc-uuid-456",
            100,
            200,
            "This is a comment",
            "commenting",
            Some("user-1"),
            "2024-01-01T00:00:00Z",
        );

        let json = anno.to_json().expect("serialize");
        assert!(json.contains("uuid-123"));
        assert!(json.contains("doc-uuid-456"));
        assert!(json.contains("TextPositionSelector"));
        assert!(json.contains("This is a comment"));

        // Round-trip
        let deserialized = Annotation::from_json(&json).expect("deserialize");
        assert_eq!(deserialized.id, "uuid-123");
    }

    #[test]
    fn test_annotation_with_dual_selectors() {
        let anno = Annotation::with_dual_selectors(
            "uuid-abc",
            "doc-xyz",
            42,
            55,
            "exact text",
            Some("prefix text"),
            Some("suffix text"),
            "my comment",
            "describing",
            None,
            "2024-06-01T12:00:00Z",
        );

        let json = anno.to_json().expect("serialize");
        assert!(json.contains("TextPositionSelector"));
        assert!(json.contains("TextQuoteSelector"));
        assert!(json.contains("exact text"));

        let deserialized = Annotation::from_json(&json).expect("deserialize");
        assert_eq!(deserialized.motivation, "describing");
    }
}
