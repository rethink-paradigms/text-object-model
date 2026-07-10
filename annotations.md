# Text Runtime — W3C Web Annotation Implementation

**Status:** Production specification
**Sources:** W3C Web Annotation Data Model (2017), Semiont (AI Alliance), STAM v0.18.7 `webanno` module, W3C Selectors and States

## Design Decision

**Custom serde types (~130 lines), not a library.**

No Rust crate provides W3C Web Annotation data types. STAM (`stam-rust`) has W3C export but builds JSON manually with `format!()` — it eschews serde because JSON-LD's polymorphic structures (body as object OR array, complex `@context`) are simpler to manage with string formatting. For our case, the annotation types are well-defined and small enough that serde with `#[serde(rename)]`, `#[serde(untagged)]`, and `serde_json::Value` for the flexible parts is cleaner than manual JSON.

## Rust Types

```rust
use serde::{Serialize, Deserialize};

// ── Selector Types ──────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TextPositionSelector {
    #[serde(rename = "type")]
    pub type_: String,                      // always "TextPositionSelector"
    pub start: usize,                       // byte offset into plain_text
    pub end: usize,                         // byte offset into plain_text
}

impl TextPositionSelector {
    pub fn new(start: usize, end: usize) -> Self {
        Self {
            type_: "TextPositionSelector".to_string(),
            start,
            end,
        }
    }

    /// Resolve against text. Returns the span if the stored position
    /// still points at matching text. This is Strategy 1 (fast path).
    pub fn resolve(&self, text: &str) -> Option<(usize, usize)> {
        if self.end <= text.len() && &text[self.start..self.end] == &text[self.start..self.end] {
            // Verification: the text at the stored position must exist
            // (self.start..self.end is always valid because we checked len)
            Some((self.start, self.end))
        } else {
            None
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TextQuoteSelector {
    #[serde(rename = "type")]
    pub type_: String,                      // always "TextQuoteSelector"
    pub exact: String,                      // the exact text of the annotated span
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,             // up to 64 chars before, word-boundary-extended
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suffix: Option<String>,             // up to 64 chars after, word-boundary-extended
}

impl TextQuoteSelector {
    pub fn new(exact: String, prefix: Option<String>, suffix: Option<String>) -> Self {
        Self {
            type_: "TextQuoteSelector".to_string(),
            exact,
            prefix,
            suffix,
        }
    }

    /// Resolve against text by searching for exact match.
    /// Uses prefix/suffix for disambiguation when exact text appears
    /// multiple times. This is the recovery path when position fails.
    pub fn resolve(&self, text: &str) -> Option<(usize, usize)> {
        let mut search_start = 0;
        loop {
            let idx = text[search_start..].find(&self.exact)?;
            let abs_idx = search_start + idx;

            if self.prefix.is_none() && self.suffix.is_none() {
                return Some((abs_idx, abs_idx + self.exact.len()));
            }

            // Check prefix context
            if let Some(ref prefix) = self.prefix {
                if abs_idx < prefix.len() {
                    search_start = abs_idx + 1;
                    continue;
                }
                let before = &text[(abs_idx - prefix.len())..abs_idx];
                if before != prefix.as_str() {
                    search_start = abs_idx + 1;
                    continue;
                }
            }

            // Check suffix context
            if let Some(ref suffix) = self.suffix {
                let end_pos = abs_idx + self.exact.len() + suffix.len();
                if end_pos > text.len() {
                    search_start = abs_idx + 1;
                    continue;
                }
                let after = &text[abs_idx + self.exact.len()..end_pos];
                if after != suffix.as_str() {
                    search_start = abs_idx + 1;
                    continue;
                }
            }

            return Some((abs_idx, abs_idx + self.exact.len()));
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]                          // because JSON is { "type": "...", ... }
pub enum Selector {
    TextPosition(TextPositionSelector),
    TextQuote(TextQuoteSelector),
    // Future: RangeSelector, FragmentSelector, etc.
}

// ── Annotation Types ────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationTarget {
    pub source: String,                     // document UUID or URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<Vec<Selector>>,    // the selectors array
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum AnnotationBody {
    TextualBody(TextualBody),
    SpecificResource(SpecificResource),
    Value(serde_json::Value),               // catch-all for any body type
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TextualBody {
    #[serde(rename = "type")]
    pub type_: String,                      // "TextualBody"
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,            // "commenting" | "tagging" | "describing"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,             // "text/plain" | "text/markdown"
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SpecificResource {
    #[serde(rename = "type")]
    pub type_: String,                      // "SpecificResource"
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Annotation {
    #[serde(rename = "@context")]
    pub context: String,                    // "http://www.w3.org/ns/anno.jsonld"
    #[serde(rename = "type")]
    pub type_: String,                      // "Annotation"

    pub id: String,                         // UUID-based IRI: "urn:uuid:..."
    pub target: AnnotationTarget,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Vec<AnnotationBody>>,  // always an array for consistency

    #[serde(skip_serializing_if = "Option::is_none")]
    pub motivation: Option<String>,         // "commenting" | "highlighting" | "tagging" | "linking"

    #[serde(skip_serializing_if = "Option::is_none")]
    pub creator: Option<String>,            // agent identifier

    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,            // ISO 8601

    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,           // ISO 8601
}

impl Annotation {
    pub fn new(id: String, target: AnnotationTarget) -> Self {
        Self {
            context: "http://www.w3.org/ns/anno.jsonld".to_string(),
            type_: "Annotation".to_string(),
            id,
            target,
            body: None,
            motivation: None,
            creator: None,
            created: Some(chrono::Utc::now().to_rfc3339()),
            modified: None,
        }
    }
}
```

## Write-Time Reconciliation

When an annotation is created, BOTH selectors (position and quote) are stored and RECONCILED to ensure they describe the same text span.

```rust
/// Create a dual-selector annotation (position + quote) from a known
/// text span. This runs at annotation write time.
///
/// The two selectors are reconciled: they MUST describe the same span.
/// If they don't (e.g., the text at the position doesn't match the quote),
/// this is a programming error — return an error.
pub fn reconcile_selectors(
    full_text: &str,
    start: usize,
    end: usize,
) -> Result<(TextPositionSelector, TextQuoteSelector), String> {
    // 1. Verify the span is valid
    if start >= end {
        return Err("start must be before end".to_string());
    }
    if end > full_text.len() {
        return Err("end exceeds text length".to_string());
    }

    let exact = &full_text[start..end];
    if exact.is_empty() {
        return Err("empty span".to_string());
    }

    // 2. Build the position selector (trivial)
    let position = TextPositionSelector::new(start, end);

    // 3. Build the quote selector with context
    // Prefix: up to 64 bytes before, extended to word boundary
    let prefix_start = start.saturating_sub(64);
    let prefix_end = start;
    let mut prefix_text = full_text[prefix_start..prefix_end].to_string();

    // Extend prefix to word boundary
    let mut actual_prefix_start = prefix_start;
    while actual_prefix_start > 0
        && !full_text.as_bytes()[actual_prefix_start - 1].is_ascii_whitespace()
    {
        actual_prefix_start -= 1;
    }
    if actual_prefix_start != prefix_start {
        prefix_text = full_text[actual_prefix_start..start].to_string();
    }

    // Suffix: up to 64 bytes after, extended to word boundary
    let suffix_start = end;
    let suffix_end = std::cmp::min(full_text.len(), end + 64);
    let mut suffix_text = full_text[suffix_start..suffix_end].to_string();

    let mut actual_suffix_end = suffix_end;
    while actual_suffix_end < full_text.len()
        && !full_text.as_bytes()[actual_suffix_end].is_ascii_whitespace()
    {
        actual_suffix_end += 1;
    }
    if actual_suffix_end != suffix_end {
        suffix_text = full_text[end..actual_suffix_end].to_string();
    }

    let quote = TextQuoteSelector::new(
        exact.to_string(),
        if prefix_text.is_empty() { None } else { Some(prefix_text) },
        if suffix_text.is_empty() { None } else { Some(suffix_text) },
    );

    // 4. RECONCILIATION: verify both selectors actually describe the same span
    let (q_start, q_end) = quote.resolve(full_text)
        .ok_or_else(|| "quote selector cannot find exact text in the document".to_string())?;

    if q_start != start || q_end != end {
        return Err(format!(
            "selector reconciliation failed: position ({},{}), quote resolved to ({},{})",
            start, end, q_start, q_end
        ));
    }

    Ok((position, quote))
}
```

## Re-Anchoring Cascade (Read Time)

When resolving an annotation against (possibly edited) text, use this cascade:

```rust
/// Resolve an annotation's selectors against (possibly edited) document text.
/// Returns the byte span (start, end) in the current document text, or None
/// if the annotation cannot be re-anchored.
///
/// Cascade strategies (from Hypothesis's production anchoring system):
///   1. Exact position match (fast path)
///   2. Quote-based search (recovers after edits)
///   (Strategies 3 and 4 — position-based + exhaustive fuzzy — added later)
pub fn resolve_annotation_span(
    full_text: &str,
    selectors: &[Selector],
) -> Option<(usize, usize)> {
    let mut position: Option<(usize, usize)> = None;
    let mut quote: Option<TextQuoteSelector> = None;

    // Extract selectors from the array
    for sel in selectors {
        match sel {
            Selector::TextPosition(p) => position = Some((p.start, p.end)),
            Selector::TextQuote(q) => quote = Some(q.clone()),
        }
    }

    // Strategy 1: Try position first (fast path)
    if let Some((start, end)) = position {
        if end <= full_text.len() {
            // Verify the text at the stored position matches
            let actual = &full_text[start..end];
            if let Some(ref q) = quote {
                if actual == q.exact {
                    return Some((start, end));
                }
            } else {
                // No quote to verify against — trust the position
                return Some((start, end));
            }
        }
    }

    // Strategy 2: Quote-based search (recovers after edits)
    if let Some(ref q) = quote {
        if let result @ Some(_) = q.resolve(full_text) {
            return result;
        }
    }

    // Strategy 3+4: reserved for future implementation
    None
}
```

## Agent Interface (STDIO)

When an agent calls `annotate`, it sends:

```json
{
    "command": "annotate",
    "doc_id": "019f...",
    "sentence": 14,
    "quote": "activation energy"
}
```

The runtime:
1. Resolves `§14` to a UUID via the session marker map
2. Loads the paragraph content file (Pandoc AST JSON)
3. Extracts plain text from the paragraph's inline array
4. Slices by `char_start`/`char_end` to get the sentence text
5. Searches for `"activation energy"` within that sentence (bounded)
6. Computes the byte offsets of the match
7. Calls `reconcile_selectors()` to build dual selectors
8. Constructs the W3C Annotation JSON-LD
9. Stores in the `annotations` table

The `quote` field is bounded to a single known sentence, which eliminates the ambiguity problem that the full W3C anchoring system has to solve with prefix/suffix disambiguation across a whole document.

## Serialized Example

```json
{
    "@context": "http://www.w3.org/ns/anno.jsonld",
    "type": "Annotation",
    "id": "urn:uuid:019f4a1b-2c3d-7e4f-8a5b-6c7d8e9f0a1b",
    "target": {
        "source": "urn:uuid:019f3a1b-2c3d-4e5f-6a7b-8c9d0e1f2a3b",
        "selector": [
            {
                "type": "TextPositionSelector",
                "start": 14,
                "end": 31
            },
            {
                "type": "TextQuoteSelector",
                "exact": "activation energy",
                "prefix": "The ",
                "suffix": " is 5.2 kJ/mol. Temperature"
            }
        ]
    },
    "body": [
        {
            "type": "TextualBody",
            "value": "This is a key claim requiring verification",
            "purpose": "commenting",
            "format": "text/plain"
        }
    ],
    "motivation": "commenting",
    "creator": "agent:web-researcher",
    "created": "2026-07-10T14:30:00Z"
}
```

## Status State Machine

```
                 ┌──────────────────────────┐
                 │         active           │
                 └────────────┬─────────────┘
                              │
                    ┌─────────┴──────────┐
                    │ re-anchor succeeds  │ (strategy 1-2)
                    ▼                    │
              ┌──────────┐               │
              │  active   │◄─────────────┘
              └──────────┘
                    │
                    │ re-anchor returns partial match (strategy 3-4, fuzzy)
                    ▼
              ┌──────────────────┐
              │ active_partial   │
              └──────────────────┘
                    │
                    │ re-anchor fails entirely
                    ▼
              ┌──────────┐
              │  orphan   │
              └──────────┘
                    │
                    │ user marks as resolved
                    ▼
              ┌──────────┐
              │  deleted  │
              └──────────┘
```

This state machine extends the W3C spec (which doesn't define annotation lifecycle). It's from the Local-First Collaboration Contract and the storage architecture. Only `active` annotations are visible in projections. `active_partial` surfaces with a warning indicator.
