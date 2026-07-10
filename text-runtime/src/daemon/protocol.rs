// ── Daemon Protocol v1 ─────────────────────────────────────────────────────
//
// Wire format: Newline-Delimited JSON (NDJSON) over a Unix domain socket.
// One JSON object per line, terminated by \n (LF).
//
// Request:  {"id":"<uuid>","cmd":"<command>","params":{...}}\n
// Response: {"id":"<uuid>","ok":true,"data":{...}}\n
//           {"id":"<uuid>","ok":false,"error":"<msg>","code":"<code>"}\n
//
// Maximum line size: 1 MiB. Connections sending oversized lines are
// disconnected with an error response.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Wire cap ─────────────────────────────────────────────────────────────────

/// Maximum NDJSON line size. Prevents slow-loris and oversized payloads.
pub const MAX_LINE_BYTES: usize = 1024 * 1024; // 1 MiB

// ── Request ───────────────────────────────────────────────────────────────────

/// Client → server request envelope.
///
/// `id` is assigned by the client and echoed in the response for
/// request/response correlation. `params` is a JSON object specific to `cmd`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    /// Client-assigned request ID (UUID v7 string recommended).
    pub id: String,
    /// Command name. Parsed into a `Cmd` by `parse_cmd`.
    pub cmd: String,
    /// Command-specific parameters (optional; defaults to null object).
    #[serde(default)]
    pub params: Value,
}

// ── Response ──────────────────────────────────────────────────────────────────

/// Server → client response envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    /// Echoed from the request `id`.
    pub id: String,
    /// `true` = success, `false` = error.
    pub ok: bool,
    /// Command-specific result data (present on success).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    /// Human-readable error message (present on failure).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Machine-readable error code (present on failure).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

impl Response {
    /// Success with no data payload.
    pub fn ok(id: &str) -> Self {
        Self {
            id: id.to_string(),
            ok: true,
            data: None,
            error: None,
            code: None,
        }
    }

    /// Success with a JSON data payload.
    pub fn ok_data(id: &str, data: Value) -> Self {
        Self {
            id: id.to_string(),
            ok: true,
            data: Some(data),
            error: None,
            code: None,
        }
    }

    /// Error response with a machine-readable code and human-readable message.
    pub fn error(id: &str, code: &str, message: &str) -> Self {
        Self {
            id: id.to_string(),
            ok: false,
            data: None,
            error: Some(message.to_string()),
            code: Some(code.to_string()),
        }
    }
}

// ── Cmd enum ─────────────────────────────────────────────────────────────────

/// All daemon commands in protocol v1.
///
/// Parsed from `Request.cmd` + `Request.params` by `parse_cmd`.
#[derive(Debug, Clone)]
pub enum Cmd {
    WorkspaceList,
    WorkspaceAdd {
        name: String,
        root: PathBuf,
        data_dir: PathBuf,
        watch_dirs: Vec<PathBuf>,
    },
    WorkspaceRemove {
        name: String,
    },
    Ingest {
        workspace: String,
        path: PathBuf,
    },
    IngestText {
        workspace: String,
        text: String,
        format: String,
        title: Option<String>,
    },
    Read {
        workspace: String,
        doc_id: String,
        format: Option<String>,
        markers: bool,
    },
    /// Annotate a sentence by UUID.
    ///
    /// `sentence_uuid` is the stable UUID of the target sentence, obtained
    /// from the `marker_map` field of a prior `Read` response.
    /// §N numbers are session-local and ephemeral — clients resolve §N → UUID
    /// themselves using the `marker_map`, then pass the UUID here.
    /// The daemon never stores per-connection §N state.
    Annotate {
        workspace: String,
        doc_id: String,
        sentence_uuid: String,
        quote: Option<String>,
        body: Option<String>,
        motivation: Option<String>,
    },
    Search {
        workspace: String,
        query: String,
        doc_id: Option<String>,
    },
    Toc {
        workspace: String,
        doc_id: String,
    },
    Status,
    Shutdown,
}

// ── parse_cmd ─────────────────────────────────────────────────────────────────

/// Parse a `Request` into a typed `Cmd`.
///
/// Returns `Err(message)` if the command name is unknown or required
/// params are missing/wrong type.
pub fn parse_cmd(req: &Request) -> Result<Cmd, String> {
    let p = &req.params;

    match req.cmd.as_str() {
        "workspace_list" => Ok(Cmd::WorkspaceList),

        "workspace_add" => Ok(Cmd::WorkspaceAdd {
            name: require_str(p, "name")?,
            root: require_path(p, "root")?,
            data_dir: require_path(p, "data_dir")?,
            watch_dirs: optional_path_array(p, "watch_dirs"),
        }),

        "workspace_remove" => Ok(Cmd::WorkspaceRemove {
            name: require_str(p, "name")?,
        }),

        "ingest" => Ok(Cmd::Ingest {
            workspace: require_str(p, "workspace")?,
            path: require_path(p, "path")?,
        }),

        "ingest_text" => Ok(Cmd::IngestText {
            workspace: require_str(p, "workspace")?,
            text: require_str(p, "text")?,
            format: require_str(p, "format")?,
            title: optional_str(p, "title"),
        }),

        "read" => Ok(Cmd::Read {
            workspace: require_str(p, "workspace")?,
            doc_id: require_str(p, "doc_id")?,
            format: optional_str(p, "format"),
            markers: optional_bool(p, "markers"),
        }),

        "annotate" => Ok(Cmd::Annotate {
            workspace: require_str(p, "workspace")?,
            doc_id: require_str(p, "doc_id")?,
            sentence_uuid: require_str(p, "sentence_uuid")?,
            quote: optional_str(p, "quote"),
            body: optional_str(p, "body"),
            motivation: optional_str(p, "motivation"),
        }),

        "search" => Ok(Cmd::Search {
            workspace: require_str(p, "workspace")?,
            query: require_str(p, "query")?,
            doc_id: optional_str(p, "doc_id"),
        }),

        "toc" => Ok(Cmd::Toc {
            workspace: require_str(p, "workspace")?,
            doc_id: require_str(p, "doc_id")?,
        }),

        "status" => Ok(Cmd::Status),
        "shutdown" => Ok(Cmd::Shutdown),

        other => Err(format!("unknown command: '{}'", other)),
    }
}

// ── Param helpers ─────────────────────────────────────────────────────────────

fn require_str(params: &Value, field: &str) -> Result<String, String> {
    params
        .get(field)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("missing required string param: '{}'", field))
}

fn require_path(params: &Value, field: &str) -> Result<PathBuf, String> {
    require_str(params, field).map(PathBuf::from)
}

fn optional_str(params: &Value, field: &str) -> Option<String> {
    params
        .get(field)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn optional_bool(params: &Value, field: &str) -> bool {
    params.get(field).and_then(|v| v.as_bool()).unwrap_or(false)
}

fn optional_path_array(params: &Value, field: &str) -> Vec<PathBuf> {
    params
        .get(field)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default()
}

// ── NDJSON framing helpers ────────────────────────────────────────────────────

/// Serialize a value to an NDJSON line (JSON bytes + `\n`).
pub fn encode_line<T: Serialize>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    let mut buf = serde_json::to_vec(value)?;
    buf.push(b'\n');
    Ok(buf)
}

/// Parse a `Request` from a raw NDJSON line (without the trailing `\n`).
pub fn parse_request(line: &str) -> Result<Request, serde_json::Error> {
    serde_json::from_str(line)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_req(cmd: &str, params: Value) -> Request {
        Request {
            id: "test-1".to_string(),
            cmd: cmd.to_string(),
            params,
        }
    }

    #[test]
    fn test_parse_request_valid() {
        let line = r#"{"id":"req-001","cmd":"status","params":{}}"#;
        let req = parse_request(line).unwrap();
        assert_eq!(req.id, "req-001");
        assert_eq!(req.cmd, "status");
    }

    #[test]
    fn test_parse_request_missing_id() {
        let line = r#"{"cmd":"status","params":{}}"#;
        // Missing id — should fail deserialization
        assert!(parse_request(line).is_err());
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let resp = Response::ok("req-1");
        let bytes = encode_line(&resp).unwrap();
        assert!(bytes.ends_with(b"\n"));
        let decoded: Response = serde_json::from_slice(&bytes[..bytes.len() - 1]).unwrap();
        assert_eq!(decoded.id, "req-1");
        assert!(decoded.ok);
    }

    #[test]
    fn test_parse_cmd_workspace_list() {
        let req = make_req("workspace_list", json!({}));
        assert!(matches!(parse_cmd(&req).unwrap(), Cmd::WorkspaceList));
    }

    #[test]
    fn test_parse_cmd_workspace_add() {
        let req = make_req(
            "workspace_add",
            json!({
                "name": "notes",
                "root": "/home/user/notes",
                "data_dir": "/home/user/.data/notes",
                "watch_dirs": ["/home/user/notes/docs"]
            }),
        );
        match parse_cmd(&req).unwrap() {
            Cmd::WorkspaceAdd {
                name, watch_dirs, ..
            } => {
                assert_eq!(name, "notes");
                assert_eq!(watch_dirs.len(), 1);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_parse_cmd_ingest() {
        let req = make_req(
            "ingest",
            json!({
                "workspace": "notes",
                "path": "/home/user/notes/doc.md"
            }),
        );
        assert!(matches!(parse_cmd(&req).unwrap(), Cmd::Ingest { .. }));
    }

    #[test]
    fn test_parse_cmd_annotate_uses_uuid() {
        let req = make_req(
            "annotate",
            json!({
                "workspace": "notes",
                "doc_id": "019f4a1b-xxxx",
                "sentence_uuid": "019f4a1b-yyyy",
                "body": "This needs a citation."
            }),
        );
        match parse_cmd(&req).unwrap() {
            Cmd::Annotate { sentence_uuid, .. } => {
                // Confirm it's a UUID string, not a u32
                assert_eq!(sentence_uuid, "019f4a1b-yyyy");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_parse_cmd_unknown() {
        let req = make_req("fly_to_moon", json!({}));
        assert!(parse_cmd(&req).is_err());
    }

    #[test]
    fn test_response_ok_serialization() {
        let resp = Response::ok_data("r1", json!({"doc_id": "abc"}));
        let s = serde_json::to_string(&resp).unwrap();
        assert!(s.contains("\"ok\":true"));
        assert!(s.contains("\"doc_id\""));
        // No error or code fields
        assert!(!s.contains("\"error\""));
        assert!(!s.contains("\"code\""));
    }

    #[test]
    fn test_response_error_serialization() {
        let resp = Response::error("r1", "WORKSPACE_NOT_FOUND", "workspace 'x' not found");
        let s = serde_json::to_string(&resp).unwrap();
        assert!(s.contains("\"ok\":false"));
        assert!(s.contains("WORKSPACE_NOT_FOUND"));
        assert!(!s.contains("\"data\""));
    }

    #[test]
    fn test_parse_cmd_status() {
        let req = make_req("status", json!({}));
        assert!(matches!(parse_cmd(&req).unwrap(), Cmd::Status));
    }

    #[test]
    fn test_parse_cmd_shutdown() {
        let req = make_req("shutdown", json!({}));
        assert!(matches!(parse_cmd(&req).unwrap(), Cmd::Shutdown));
    }

    #[test]
    fn test_parse_cmd_search() {
        let req = make_req(
            "search",
            json!({
                "workspace": "notes",
                "query": "quantum entanglement"
            }),
        );
        match parse_cmd(&req).unwrap() {
            Cmd::Search { query, doc_id, .. } => {
                assert_eq!(query, "quantum entanglement");
                assert!(doc_id.is_none());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_parse_cmd_read() {
        let req = make_req(
            "read",
            json!({
                "workspace": "notes",
                "doc_id": "019f4a1b-xxxx",
                "markers": true
            }),
        );
        match parse_cmd(&req).unwrap() {
            Cmd::Read {
                markers, format, ..
            } => {
                assert!(markers);
                assert!(format.is_none());
            }
            _ => panic!("wrong variant"),
        }
    }
}
