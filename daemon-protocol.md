# Daemon Protocol v1 — Text Runtime

**Date:** 2026-07-10 (Updated: 2026-07-11)
**Status:** Implemented (Phase 2 Daemon)
**Wire format:** Newline-Delimited JSON (NDJSON)
**Transport:** Unix domain socket (`~/.local/state/text-runtime/runtime.sock`)

---

## 1. Wire Format

One JSON object per line, terminated by `\n` (LF, byte 0x0A).

**Request (client → server):**
```json
{"id":"<uuid>","cmd":"<command>","params":{...}}
```

**Response (server → client):**
```json
{"id":"<uuid>","ok":true,"data":{...}}
```
```json
{"id":"<uuid>","ok":false,"error":"<message>","code":"<error_code>"}
```

**Maximum line size:** 1,048,576 bytes (1 MiB). Connections sending lines exceeding this cap will be disconnected with an error response.

---

## 2. Envelope Fields

### Request

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string (UUID v7) | yes | Client-assigned request ID, echoed in response |
| `cmd` | string | yes | Command name (see §3) |
| `params` | object | no | Command-specific parameters |

### Response

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | yes | Echoed from request |
| `ok` | boolean | yes | `true` = success, `false` = error |
| `data` | object | on success | Command-specific result data |
| `error` | string | on failure | Human-readable error message |
| `code` | string | on failure | Machine-readable error code |

---

## 3. Commands

### 3.1 `workspace_list`

List all active workspaces and their status.

**Request:**
```json
{"id":"req-001","cmd":"workspace_list","params":{}}
```

**Response:**
```json
{
  "id":"req-001",
  "ok":true,
  "data":{
    "workspaces": [
      {"name":"notes","root":"/home/user/notes","status":"active"},
      {"name":"work","root":"/home/user/work-docs","status":"active"}
    ]
  }
}
```

---

### 3.2 `workspace_add`

Add a new workspace at runtime.

**Request:**
```json
{
  "id":"req-002",
  "cmd":"workspace_add",
  "params":{
    "name":"blog",
    "root":"/home/user/blog",
    "data_dir":"/home/user/.local/share/text-runtime/blog",
    "watch_dirs":["/home/user/blog/posts"]
  }
}
```

**Response (success):**
```json
{"id":"req-002","ok":true}
```

**Response (already exists):**
```json
{"id":"req-002","ok":false,"error":"workspace 'blog' already exists","code":"WORKSPACE_EXISTS"}
```

---

### 3.3 `workspace_remove`

Remove a workspace. In-flight operations complete; new requests for this workspace return an error.

**Request:**
```json
{"id":"req-003","cmd":"workspace_remove","params":{"name":"blog"}}
```

**Response (success):**
```json
{"id":"req-003","ok":true}
```

**Response (not found):**
```json
{"id":"req-003","ok":false,"error":"workspace 'blog' not found","code":"WORKSPACE_NOT_FOUND"}
```

---

### 3.4 `ingest`

Ingest a file from disk into a workspace.

**Request:**
```json
{
  "id":"req-004",
  "cmd":"ingest",
  "params":{
    "workspace":"notes",
    "path":"/home/user/notes/physics.md"
  }
}
```

**Response (success):**
```json
{
  "id":"req-004",
  "ok":true,
  "data":{
    "doc_id":"019f4a1b-2c3d-7e4f-8a5b-6c7d8e9f0a1b"
  }
}
```

**Response (file not found):**
```json
{"id":"req-004","ok":false,"error":"I/O error at '/home/user/notes/physics.md': No such file or directory","code":"INGEST_ERROR"}
```

---

### 3.5 `ingest_text`

Ingest raw text directly (no file on disk).

**Request:**
```json
{
  "id":"req-005",
  "cmd":"ingest_text",
  "params":{
    "workspace":"notes",
    "text":"# Hello World\n\nThis is a test document.\n",
    "format":"markdown",
    "title":"Hello World"
  }
}
```

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `workspace` | string | yes | Workspace name |
| `text` | string | yes | Raw text content |
| `format` | string | yes | Pandoc format: `markdown`, `latex`, `html`, `plain`, etc. |
| `title` | string | no | Document title (defaults to "untitled") |

**Response (success):**
```json
{
  "id":"req-005",
  "ok":true,
  "data":{
    "doc_id":"019f4a1b-2c3d-7e4f-8a5b-6c7d8e9f0b2c"
  }
}
```

---

### 3.6 `read`

Project (read) a document, optionally with §N sentence markers.

**Request:**
```json
{
  "id":"req-006",
  "cmd":"read",
  "params":{
    "workspace":"notes",
    "doc_id":"019f4a1b-2c3d-7e4f-8a5b-6c7d8e9f0a1b",
    "format":"markdown",
    "markers":true
  }
}
```

| Param | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `workspace` | string | yes | - | Workspace name |
| `doc_id` | string | yes | - | Document UUID |
| `format` | string | no | `"markdown"` | Output format |
| `markers` | boolean | no | `false` | Inject §N sentence markers |

**Response (with markers):**
```json
{
  "id":"req-006",
  "ok":true,
  "data":{
    "text":"§1 The activation energy of this reaction is 5.2 kJ/mol.\n§2 Temperature plays a critical role.\n§3 This suggests further study.\n",
    "format":"markdown",
    "marker_map":{"1":"019f4a1b-xxxx","2":"019f4a1b-yyyy","3":"019f4a1b-zzzz"}
  }
}
```

**Response (without markers):**
```json
{
  "id":"req-006",
  "ok":true,
  "data":{
    "text":"The activation energy of this reaction is 5.2 kJ/mol.\nTemperature plays a critical role.\nThis suggests further study.\n",
    "format":"markdown"
  }
}
```

---

### 3.7 `annotate`

Create an annotation targeting a sentence.

> **Note on sentence addressing:** §N marker numbers are session-local and ephemeral — they are only valid within a single `read` call's response. Clients must resolve §N → UUID using the `marker_map` returned by `read`, then pass the UUID here as `sentence_uuid`. This keeps the daemon stateless with respect to client sessions.

**Request (whole sentence):**
```json
{
  "id":"req-007",
  "cmd":"annotate",
  "params":{
    "workspace":"notes",
    "doc_id":"019f4a1b-2c3d-7e4f-8a5b-6c7d8e9f0a1b",
    "sentence_uuid":"019f4a1b-2c3d-7e4f-8a5b-6c7d8e9f0001",
    "body":"This needs a citation.",
    "motivation":"commenting"
  }
}
```

**Request (sub-sentence quote):**
```json
{
  "id":"req-008",
  "cmd":"annotate",
  "params":{
    "workspace":"notes",
    "doc_id":"019f4a1b-2c3d-7e4f-8a5b-6c7d8e9f0a1b",
    "sentence_uuid":"019f4a1b-2c3d-7e4f-8a5b-6c7d8e9f0001",
    "quote":"activation energy",
    "body":"Define this term.",
    "motivation":"commenting"
  }
}
```

| Param | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `workspace` | string | yes | - | Workspace name |
| `doc_id` | string | yes | - | Document UUID |
| `sentence_uuid` | string | yes | - | UUID of the target sentence. Obtain from `marker_map` in a prior `read` response. |
| `quote` | string | no | - | Exact quote text within the sentence |
| `body` | string | no | `""` | Annotation body text |
| `motivation` | string | no | `"commenting"` | W3C motivation: `commenting`, `describing`, `tagging`, `highlighting`, `bookmarking`, `questioning`, `editing` |

**Response:**
```json
{
  "id":"req-007",
  "ok":true,
  "data":{
    "annotation_id":"019f4a1b-2c3d-7e4f-8a5b-6c7d8e9f0a0a"
  }
}
```

---

### 3.8 `search`

Full-text search across documents in a workspace.

**Request (all documents):**
```json
{
  "id":"req-009",
  "cmd":"search",
  "params":{
    "workspace":"notes",
    "query":"quantum entanglement"
  }
}
```

**Request (scoped to document):**
```json
{
  "id":"req-010",
  "cmd":"search",
  "params":{
    "workspace":"notes",
    "query":"quantum",
    "doc_id":"019f4a1b-2c3d-7e4f-8a5b-6c7d8e9f0a1b"
  }
}
```

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `workspace` | string | yes | Workspace name |
| `query` | string | yes | FTS5 search query (sanitized internally) |
| `doc_id` | string | no | Scope search to a specific document |

**Response:**
```json
{
  "id":"req-009",
  "ok":true,
  "data":{
    "hits":[
      {
        "uuid":"019f4a1b-xxxx",
        "node_type":"sentence",
        "doc_id":"019f4a1b-2c3d-7e4f-8a5b-6c7d8e9f0a1b",
        "snippet":"The <mark>quantum</mark> <mark>entanglement</mark> hypothesis has been debated for decades.",
        "score":-2.451
      },
      {
        "uuid":"019f4a1b-yyyy",
        "node_type":"paragraph",
        "doc_id":"019f4a1b-zzzz",
        "snippet":"<mark>Quantum</mark> computers exploit <mark>entanglement</mark> for parallel computation.",
        "score":-1.893
      }
    ]
  }
}
```

Hits are ordered by BM25 score (lower = better match). Snippets use `<mark>` tags for highlight terms.

---

### 3.9 `toc`

Get the table of contents (all headings) for a document.

**Request:**
```json
{
  "id":"req-011",
  "cmd":"toc",
  "params":{
    "workspace":"notes",
    "doc_id":"019f4a1b-2c3d-7e4f-8a5b-6c7d8e9f0a1b"
  }
}
```

**Response:**
```json
{
  "id":"req-011",
  "ok":true,
  "data":{
    "entries":[
      {"uuid":"uuid-h1","plain_text":"Introduction","heading_level":1,"section_path":"1","position":1000.0},
      {"uuid":"uuid-h2","plain_text":"Methods","heading_level":2,"section_path":"1.1","position":2000.0},
      {"uuid":"uuid-h3","plain_text":"Results","heading_level":2,"section_path":"1.2","position":3000.0}
    ]
  }
}
```

---

### 3.10 `status`

Get daemon health and version info.

**Request:**
```json
{"id":"req-012","cmd":"status","params":{}}
```

**Response:**
```json
{
  "id":"req-012",
  "ok":true,
  "data":{
    "status": "running",
    "version": "0.1.0",
    "uptime_seconds": 8472,
    "workspaces": 3
  }
}
```

---

### 3.11 `shutdown`

Instruct the daemon to begin graceful shutdown.

**Request:**
```json
{"id":"req-013","cmd":"shutdown","params":{}}
```

**Response:**
```json
{"id":"req-013","ok":true}
```

The daemon acknowledges immediately, then begins draining connections. The socket is unlinked when shutdown completes.

---

## 4. Error Codes

| Code | Meaning |
|------|---------|
| `BAD_REQUEST` | Malformed request (invalid JSON, missing cmd, etc.) |
| `UNKNOWN_CMD` | Command not recognized |
| `WORKSPACE_NOT_FOUND` | Requested workspace does not exist |
| `WORKSPACE_EXISTS` | Workspace already exists (on add) |
| `INGEST_ERROR` | File read failed, parse error, or pipeline failure |
| `READ_ERROR` | Document not found or projection failed |
| `ANNOTATE_ERROR` | Sentence marker not found, quote mismatch, or insert failure |
| `SEARCH_ERROR` | FTS5 query error |
| `TOC_ERROR` | Document not found or heading retrieval failed |
| `INTERNAL_ERROR` | Unexpected internal failure (details in error message) |

---

## 5. Connection Lifecycle

1. **Client connects** to `~/.local/state/text-runtime/runtime.sock` via `UnixStream`
2. **Client sends** one NDJSON request line
3. **Server processes** the request and sends one NDJSON response line
4. **Client reads** the response
5. Connection may be kept open for pipelining (client sends multiple requests on same connection) or closed after one request/response pair
6. **Server closes** the connection on parse error (line too long, invalid JSON) — the error is sent before close

**Pipelining:** Multiple requests can be sent on one connection. Responses arrive in order. The server reads one line at a time from the stream, processes it, writes the response, then reads the next. **Requests on a single connection are processed sequentially** — a slow `ingest` will block subsequent requests on the same connection until it completes. For concurrent operations, use separate connections.

**Shutdown behavior:** During graceful shutdown, the server stops accepting new connections. Existing connections continue processing until they complete or the shutdown grace period expires.

---

## 6. Building a Client

### Language-agnostic notes

Any language with Unix socket support and a JSON library can be a client. The only requirements:

1. Open a Unix stream socket at the socket path
2. Write a JSON object as a single line ending with `\n`
3. Read one line back — parse as JSON

### Rust client (pseudocode)

```rust
use tokio::net::UnixStream;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

let mut stream = UnixStream::connect("~/.local/state/text-runtime/runtime.sock").await?;
let (reader, mut writer) = stream.split();

// Send request
let req = serde_json::json!({
    "id": "cli-001",
    "cmd": "status",
    "params": {}
});
let mut line = serde_json::to_string(&req)?;
line.push('\n');
writer.write_all(line.as_bytes()).await?;

// Read response
let mut buf_reader = BufReader::new(reader);
let mut response_line = String::new();
buf_reader.read_line(&mut response_line).await?;
let resp: serde_json::Value = serde_json::from_str(&response_line)?;
```

### TypeScript/Node.js client (pseudocode)

```typescript
import * as net from 'net';
import * as os from 'os';

const socketPath = `${os.homedir()}/.local/state/text-runtime/runtime.sock`;

const client = net.createConnection(socketPath);

client.on('connect', () => {
  const req = JSON.stringify({ id: 'ts-001', cmd: 'status', params: {} }) + '\n';
  client.write(req);
});

let buffer = '';
client.on('data', (data) => {
  buffer += data.toString();
  const newlineIndex = buffer.indexOf('\n');
  if (newlineIndex !== -1) {
    const line = buffer.substring(0, newlineIndex);
    const response = JSON.parse(line);
    console.log(response);
    client.end();
  }
});
```

### Debugging with netcat

```bash
# See if daemon is alive
echo '{"id":"nc-001","cmd":"status","params":{}}' | nc -U ~/.local/state/text-runtime/runtime.sock

# Ingest a document
echo '{"id":"nc-002","cmd":"ingest","params":{"workspace":"notes","path":"~/notes/test.md"}}' | nc -U ~/.local/state/text-runtime/runtime.sock

# Search
echo '{"id":"nc-003","cmd":"search","params":{"workspace":"notes","query":"quantum"}}' | nc -U ~/.local/state/text-runtime/runtime.sock | jq .
```

---

## 7. Future Extensions

### Content-Length framing (for MCP/LSP compatibility)

An optional alternative framing: instead of NDJSON, use `Content-Length: N\r\n\r\n<JSON>` headers. This would make the daemon compatible with Model Context Protocol clients and VS Code extensions that use LSP framing. The daemon can auto-detect the framing by reading the first byte:

- `{` → NDJSON
- `C` → Content-Length headers

### Streaming responses

For long-running operations (large document projection), the server could stream partial results via multiple response lines. This requires protocol extensions (event/end markers) and is out of scope for v1.

### Notifications (server → client push)

The daemon could push events to connected clients: file changes detected, workspace status changes, etc. This requires clients to keep a persistent connection open. Out of scope for v1.



