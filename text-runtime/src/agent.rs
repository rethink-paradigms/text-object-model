// ── STDIO Agent Protocol ────────────────────────────────────────────────────
//
// Reads JSON commands from stdin, dispatches to the Runtime, writes JSON
// responses to stdout. This is the primary interface for external tools
// (AI agents, editors, CLI wrappers).
//
// Protocol:
//   → {"command": "ingest", "path": "/doc.md"}
//   ← {"ok": true, "doc_id": "019f..."}
//
//   → {"command": "read", "doc_id": "019f...", "format": "markdown", "markers": true}
//   ← {"ok": true, "text": "# Hello\n\n§1 This is...", "markers": {"1": "uuid-1"}}
//
//   → {"command": "annotate", "doc_id": "...", "sentence_uuid": "uuid-1", "body": "check this"}
//   ← {"ok": true, "annotation_id": "019f..."}
//
//   → {"command": "search", "query": "hello"}
//   ← {"ok": true, "hits": [...]}
//
//   → {"command": "transclude", "source": "...", "target": "...", "predicate": "transcludes"}
//   ← {"ok": true, "edge_id": "019f..."}
//
//   → {"command": "toc", "doc_id": "..."}
//   ← {"ok": true, "entries": [...]}
//
//   → {"command": "close"}
//   ← {"ok": true}

use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::TextRuntimeError;
use crate::runtime::Runtime;

/// An incoming agent command.
#[derive(Debug, Deserialize)]
#[serde(tag = "command")]
enum AgentCommand {
    /// Ingest a file from disk.
    #[serde(rename = "ingest")]
    Ingest { path: String },

    /// Read/project a document.
    #[serde(rename = "read")]
    Read {
        doc_id: String,
        #[serde(default)]
        format: Option<String>,
        #[serde(default)]
        markers: bool,
    },

    /// Create an annotation.
    ///
    /// `sentence_uuid` is the UUID of the target sentence, obtained from the
    /// `markers` map returned by a prior `read` command with `markers: true`.
    #[serde(rename = "annotate")]
    Annotate {
        doc_id: String,
        sentence_uuid: String,
        #[serde(default)]
        quote: Option<String>,
        #[serde(default)]
        body: Option<String>,
        #[serde(default)]
        motivation: Option<String>,
    },

    /// Search documents.
    #[serde(rename = "search")]
    Search {
        query: String,
        #[serde(default)]
        doc_id: Option<String>,
    },

    /// Create a transclusion edge.
    #[serde(rename = "transclude")]
    Transclude {
        source: String,
        target: String,
        predicate: String,
    },

    /// Get table of contents.
    #[serde(rename = "toc")]
    Toc { doc_id: String },

    /// Shutdown the agent.
    #[serde(rename = "close")]
    Close,
}

/// An outgoing agent response.
#[derive(Debug, Serialize)]
struct AgentResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    doc_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    markers: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    annotation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hits: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    edge_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    entries: Option<Vec<serde_json::Value>>,
}

impl AgentResponse {
    fn ok() -> Self {
        Self {
            ok: true,
            error: None,
            doc_id: None,
            text: None,
            format: None,
            markers: None,
            annotation_id: None,
            hits: None,
            edge_id: None,
            entries: None,
        }
    }

    fn error(msg: &str) -> Self {
        Self {
            ok: false,
            error: Some(msg.to_string()),
            doc_id: None,
            text: None,
            format: None,
            markers: None,
            annotation_id: None,
            hits: None,
            edge_id: None,
            entries: None,
        }
    }
}

/// Run the agent protocol loop.
///
/// Reads JSON commands from stdin, dispatches to the Runtime, writes
/// JSON responses to stdout. Runs until a `close` command is received
/// or stdin is closed.
pub async fn run_agent_loop(runtime: &mut Runtime) -> Result<(), TextRuntimeError> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();
    let mut line = String::new();

    loop {
        line.clear();

        // Read a line from stdin
        match reader.read_line(&mut line) {
            Ok(0) => {
                // EOF — clean exit
                break;
            }
            Ok(_) => {}
            Err(e) => {
                let resp = AgentResponse::error(&format!("I/O error reading stdin: {}", e));
                let json = serde_json::to_string(&resp).unwrap_or_default();
                let _ = writeln!(writer, "{}", json);
                let _ = writer.flush();
                break;
            }
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Parse the command
        let command: AgentCommand = match serde_json::from_str(trimmed) {
            Ok(cmd) => cmd,
            Err(e) => {
                let resp = AgentResponse::error(&format!("invalid command: {}", e));
                let json = serde_json::to_string(&resp).unwrap_or_default();
                let _ = writeln!(writer, "{}", json);
                let _ = writer.flush();
                continue;
            }
        };

        // Check for close before dispatching
        let is_close = matches!(command, AgentCommand::Close);

        // Dispatch to Runtime
        let response = handle_command(runtime, command).await;

        // Write response
        let json = serde_json::to_string(&response).unwrap_or_else(|_| {
            r#"{"ok":false,"error":"failed to serialize response"}"#.to_string()
        });
        let _ = writeln!(writer, "{}", json);
        let _ = writer.flush();

        if is_close {
            break;
        }
    }

    Ok(())
}

/// Handle a single agent command.
async fn handle_command(runtime: &mut Runtime, command: AgentCommand) -> AgentResponse {
    match command {
        AgentCommand::Ingest { path } => {
            let path = PathBuf::from(&path);
            match runtime.ingest_file(&path).await {
                Ok(doc_id) => {
                    let mut resp = AgentResponse::ok();
                    resp.doc_id = Some(doc_id);
                    resp
                }
                Err(e) => AgentResponse::error(&e.to_string()),
            }
        }

        AgentCommand::Read {
            doc_id,
            format,
            markers,
        } => {
            let fmt = format.as_deref().unwrap_or("markdown");
            match runtime.read(&doc_id, fmt, markers) {
                Ok(projection) => {
                    let mut resp = AgentResponse::ok();
                    resp.text = Some(projection.text);
                    resp.format = Some(projection.format);
                    if let Some(map) = projection.marker_map {
                        let json_map: serde_json::Map<String, serde_json::Value> = map
                            .into_iter()
                            .map(|(k, v)| (k.to_string(), serde_json::Value::String(v)))
                            .collect();
                        resp.markers = Some(serde_json::Value::Object(json_map));
                    }
                    resp
                }
                Err(e) => AgentResponse::error(&e.to_string()),
            }
        }

        AgentCommand::Annotate {
            doc_id,
            sentence_uuid,
            quote,
            body,
            motivation,
        } => {
            match runtime.annotate_by_uuid(
                &doc_id,
                &sentence_uuid,
                quote.as_deref(),
                body.as_deref(),
                motivation.as_deref(),
            ) {
                Ok(anno_id) => {
                    let mut resp = AgentResponse::ok();
                    resp.annotation_id = Some(anno_id);
                    resp
                }
                Err(e) => AgentResponse::error(&e.to_string()),
            }
        }

        AgentCommand::Search { query, doc_id } => match runtime.search(&query, doc_id.as_deref()) {
            Ok(hits) => {
                let json_hits: Vec<serde_json::Value> = hits
                    .into_iter()
                    .map(|h| {
                        serde_json::json!({
                            "uuid": h.uuid,
                            "node_type": h.node_type,
                            "doc_id": h.doc_id,
                            "snippet": h.snippet,
                            "score": h.score,
                        })
                    })
                    .collect();
                let mut resp = AgentResponse::ok();
                resp.hits = Some(json_hits);
                resp
            }
            Err(e) => AgentResponse::error(&e.to_string()),
        },

        AgentCommand::Transclude {
            source,
            target,
            predicate,
        } => match runtime.transclude(&source, &target, &predicate) {
            Ok(edge_id) => {
                let mut resp = AgentResponse::ok();
                resp.edge_id = Some(edge_id);
                resp
            }
            Err(e) => AgentResponse::error(&e.to_string()),
        },

        AgentCommand::Toc { doc_id } => match runtime.toc(&doc_id) {
            Ok(entries) => {
                let json_entries: Vec<serde_json::Value> = entries
                    .into_iter()
                    .map(|e| {
                        serde_json::json!({
                            "uuid": e.uuid,
                            "plain_text": e.plain_text,
                            "heading_level": e.heading_level,
                            "section_path": e.section_path,
                            "position": e.position,
                        })
                    })
                    .collect();
                let mut resp = AgentResponse::ok();
                resp.entries = Some(json_entries);
                resp
            }
            Err(e) => AgentResponse::error(&e.to_string()),
        },

        AgentCommand::Close => AgentResponse::ok(),
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_response_serialization() {
        let mut resp = AgentResponse::ok();
        resp.doc_id = Some("test-uuid".to_string());
        let json = serde_json::to_string(&resp).expect("serialize");
        assert!(json.contains("\"ok\":true"));
        assert!(json.contains("\"doc_id\":\"test-uuid\""));
    }

    #[test]
    fn test_error_response() {
        let resp = AgentResponse::error("something went wrong");
        let json = serde_json::to_string(&resp).expect("serialize");
        assert!(json.contains("\"ok\":false"));
        assert!(json.contains("something went wrong"));
    }

    #[test]
    fn test_command_deserialization_ingest() {
        let json = r#"{"command": "ingest", "path": "/tmp/test.md"}"#;
        let cmd: AgentCommand = serde_json::from_str(json).expect("deserialize");
        match cmd {
            AgentCommand::Ingest { path } => assert_eq!(path, "/tmp/test.md"),
            _ => panic!("wrong command variant"),
        }
    }

    #[test]
    fn test_command_deserialization_read() {
        let json =
            r#"{"command": "read", "doc_id": "uuid-123", "format": "html", "markers": true}"#;
        let cmd: AgentCommand = serde_json::from_str(json).expect("deserialize");
        match cmd {
            AgentCommand::Read {
                doc_id,
                format,
                markers,
            } => {
                assert_eq!(doc_id, "uuid-123");
                assert_eq!(format, Some("html".to_string()));
                assert!(markers);
            }
            _ => panic!("wrong command variant"),
        }
    }

    #[test]
    fn test_command_deserialization_search() {
        let json = r#"{"command": "search", "query": "hello world"}"#;
        let cmd: AgentCommand = serde_json::from_str(json).expect("deserialize");
        match cmd {
            AgentCommand::Search { query, doc_id } => {
                assert_eq!(query, "hello world");
                assert!(doc_id.is_none());
            }
            _ => panic!("wrong command variant"),
        }
    }
}
