// ── Daemon Request Handler ─────────────────────────────────────────────────
//
// Implements the Handler trait: dispatches parsed Cmd variants to the
// appropriate Runtime operations via the WorkspaceRegistry.
//
// Lock discipline (from workspace-registry.ts lego):
//   1. registry.get(name)  →  clones Arc<WorkspaceHandle>, drops DashMap ref
//   2. handle.runtime.lock().await  →  lock workspace Mutex
//   3. perform operation
//   4. drop Mutex guard (end of block)
//   Never hold DashMap ref across .await. Never hold two workspace Mutex locks.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::error::TextRuntimeError;
use crate::runtime::IngestMetadata;

use super::protocol::{parse_cmd, Cmd, Request, Response};
use super::registry::WorkspaceRegistry;
use super::watcher::spawn_watcher;
use super::workspace::WorkspaceHandle;

// ── Handler trait ─────────────────────────────────────────────────────────────

/// Async request dispatcher.
#[async_trait]
pub trait Handler: Send + Sync + 'static {
    async fn dispatch(&self, req: Request) -> Response;
}

// ── DaemonHandler ─────────────────────────────────────────────────────────────

/// The production handler: owns registry, daemon metadata, and shutdown token.
pub struct DaemonHandler {
    pub registry: Arc<WorkspaceRegistry>,
    pub start_time: Instant,
    pub version: String,
    pub shutdown_token: CancellationToken,
}

impl DaemonHandler {
    pub fn new(registry: Arc<WorkspaceRegistry>, shutdown_token: CancellationToken) -> Self {
        Self {
            registry,
            start_time: Instant::now(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            shutdown_token,
        }
    }
}

#[async_trait]
impl Handler for DaemonHandler {
    async fn dispatch(&self, req: Request) -> Response {
        let id = req.id.clone();
        match parse_cmd(&req) {
            Err(msg) => Response::error(&id, "BAD_CMD", &msg),
            Ok(cmd) => self.route(cmd, &id).await,
        }
    }
}

impl DaemonHandler {
    async fn route(&self, cmd: Cmd, id: &str) -> Response {
        match cmd {
            Cmd::WorkspaceList => self.handle_workspace_list(id),
            Cmd::WorkspaceAdd {
                name,
                root,
                data_dir,
                watch_dirs,
            } => {
                self.handle_workspace_add(id, name, root, data_dir, watch_dirs)
                    .await
            }
            Cmd::WorkspaceRemove { name } => self.handle_workspace_remove(id, name).await,
            Cmd::Ingest { workspace, path } => self.handle_ingest(id, &workspace, path).await,
            Cmd::IngestText {
                workspace,
                text,
                format,
                title,
            } => {
                self.handle_ingest_text(id, &workspace, text, format, title)
                    .await
            }
            Cmd::Read {
                workspace,
                doc_id,
                format,
                markers,
            } => {
                self.handle_read(id, &workspace, doc_id, format, markers)
                    .await
            }
            Cmd::Annotate {
                workspace,
                doc_id,
                sentence_uuid,
                quote,
                body,
                motivation,
            } => {
                self.handle_annotate(
                    id,
                    &workspace,
                    doc_id,
                    sentence_uuid,
                    quote,
                    body,
                    motivation,
                )
                .await
            }
            Cmd::Search {
                workspace,
                query,
                doc_id,
            } => self.handle_search(id, &workspace, query, doc_id).await,
            Cmd::Toc { workspace, doc_id } => self.handle_toc(id, &workspace, doc_id).await,
            Cmd::Status => self.handle_status(id),
            Cmd::Shutdown => self.handle_shutdown(id),
        }
    }

    // ── workspace_list ──────────────────────────────────────────────────────

    fn handle_workspace_list(&self, id: &str) -> Response {
        let workspaces: Vec<Value> = self
            .registry
            .list()
            .into_iter()
            .map(|info| {
                json!({
                    "name": info.name,
                    "root": info.root.to_string_lossy(),
                    "status": info.status,
                })
            })
            .collect();
        Response::ok_data(id, json!({ "workspaces": workspaces }))
    }

    // ── workspace_add ───────────────────────────────────────────────────────

    async fn handle_workspace_add(
        &self,
        id: &str,
        name: String,
        root: std::path::PathBuf,
        data_dir: std::path::PathBuf,
        watch_dirs: Vec<std::path::PathBuf>,
    ) -> Response {
        let handle = match WorkspaceHandle::open(name.clone(), root, data_dir).await {
            Ok(h) => h,
            Err(e) => return Response::error(id, "WORKSPACE_OPEN_ERROR", &e.to_string()),
        };

        // Spawn watcher if watch_dirs were provided
        if !watch_dirs.is_empty() {
            let cancel_token = handle.watcher_token.clone();
            // We need an Arc to pass to the watcher, but we haven't inserted yet.
            // Insert first, then get Arc back and spawn watcher.
            if let Err(e) = self.registry.insert(handle) {
                return Response::error(id, "WORKSPACE_EXISTS", &e.to_string());
            }
            let arc = self.registry.get(&name).unwrap();
            let task = spawn_watcher(arc, watch_dirs, 500, cancel_token);
            // Store the task handle in the workspace
            if let Some(ws) = self.registry.get(&name) {
                *ws.watcher_task.lock().await = Some(task);
            }
        } else {
            if let Err(e) = self.registry.insert(handle) {
                return Response::error(id, "WORKSPACE_EXISTS", &e.to_string());
            }
        }

        Response::ok_data(id, json!({ "name": name }))
    }

    // ── workspace_remove ────────────────────────────────────────────────────

    async fn handle_workspace_remove(&self, id: &str, name: String) -> Response {
        let arc = match self.registry.remove(&name) {
            Some(a) => a,
            None => {
                return Response::error(
                    id,
                    "WORKSPACE_NOT_FOUND",
                    &format!("workspace '{}' not found", name),
                )
            }
        };

        // Try to take sole ownership — if in-flight handlers hold refs,
        // they'll finish and the workspace auto-cleans when the last Arc drops.
        match Arc::try_unwrap(arc) {
            Ok(owned) => {
                if let Err(e) = owned.shutdown().await {
                    return Response::error(id, "WORKSPACE_CLOSE_ERROR", &e.to_string());
                }
            }
            Err(still_alive) => {
                // Cancel watcher so it stops watching; runtime cleans up when
                // the last holder drops.
                still_alive.watcher_token.cancel();
            }
        }

        Response::ok_data(id, json!({ "removed": name }))
    }

    // ── ingest ──────────────────────────────────────────────────────────────

    async fn handle_ingest(&self, id: &str, workspace: &str, path: std::path::PathBuf) -> Response {
        let handle = match self.registry.get(workspace) {
            Some(h) => h,
            None => {
                return Response::error(
                    id,
                    "WORKSPACE_NOT_FOUND",
                    &format!("workspace '{}' not found", workspace),
                )
            }
        };

        let result = {
            let mut rt = handle.runtime.lock().await;
            rt.ingest_file(&path).await
        };

        match result {
            Ok(doc_id) => Response::ok_data(id, json!({ "doc_id": doc_id })),
            Err(TextRuntimeError::IoError { path: p, source: e }) => Response::error(
                id,
                "IO_ERROR",
                &format!("cannot read {}: {}", p.display(), e),
            ),
            Err(e) => Response::error(id, "INGEST_ERROR", &e.to_string()),
        }
    }

    // ── ingest_text ─────────────────────────────────────────────────────────

    async fn handle_ingest_text(
        &self,
        id: &str,
        workspace: &str,
        text: String,
        format: String,
        title: Option<String>,
    ) -> Response {
        let handle = match self.registry.get(workspace) {
            Some(h) => h,
            None => {
                return Response::error(
                    id,
                    "WORKSPACE_NOT_FOUND",
                    &format!("workspace '{}' not found", workspace),
                )
            }
        };

        let result = {
            let mut rt = handle.runtime.lock().await;
            let meta = IngestMetadata {
                title,
                source_path: None,
                language: None,
            };
            rt.ingest_text(&text, &format, &meta).await
        };

        match result {
            Ok(doc_id) => Response::ok_data(id, json!({ "doc_id": doc_id })),
            Err(e) => Response::error(id, "INGEST_ERROR", &e.to_string()),
        }
    }

    // ── read ────────────────────────────────────────────────────────────────

    async fn handle_read(
        &self,
        id: &str,
        workspace: &str,
        doc_id: String,
        format: Option<String>,
        markers: bool,
    ) -> Response {
        let handle = match self.registry.get(workspace) {
            Some(h) => h,
            None => {
                return Response::error(
                    id,
                    "WORKSPACE_NOT_FOUND",
                    &format!("workspace '{}' not found", workspace),
                )
            }
        };

        let fmt = format.as_deref().unwrap_or("plain");

        let result = {
            let rt = handle.runtime.lock().await;
            rt.read(&doc_id, fmt, markers)
        };

        match result {
            Ok(proj) => {
                let mut data = json!({
                    "doc_id": doc_id,
                    "format": proj.format,
                    "text": proj.text,
                });
                // Include marker_map if markers were requested
                // The client is responsible for holding this map and resolving
                // §N → UUID before calling annotate.
                if let Some(map) = proj.marker_map {
                    let json_map: serde_json::Map<String, Value> = map
                        .into_iter()
                        .map(|(k, v)| (k.to_string(), json!(v)))
                        .collect();
                    data["marker_map"] = Value::Object(json_map);
                }
                Response::ok_data(id, data)
            }
            Err(TextRuntimeError::DocumentNotFound(d)) => Response::error(
                id,
                "DOCUMENT_NOT_FOUND",
                &format!("document '{}' not found", d),
            ),
            Err(e) => Response::error(id, "READ_ERROR", &e.to_string()),
        }
    }

    // ── annotate ────────────────────────────────────────────────────────────

    // Request fields are a cohesive set; a request struct is tracked as future
    // debt. (clippy::too_many_arguments)
    #[allow(clippy::too_many_arguments)]
    async fn handle_annotate(
        &self,
        id: &str,
        workspace: &str,
        doc_id: String,
        sentence_uuid: String,
        quote: Option<String>,
        body: Option<String>,
        motivation: Option<String>,
    ) -> Response {
        let handle = match self.registry.get(workspace) {
            Some(h) => h,
            None => {
                return Response::error(
                    id,
                    "WORKSPACE_NOT_FOUND",
                    &format!("workspace '{}' not found", workspace),
                )
            }
        };

        let result = {
            let rt = handle.runtime.lock().await;
            rt.annotate_by_uuid(
                &doc_id,
                &sentence_uuid,
                quote.as_deref(),
                body.as_deref(),
                motivation.as_deref(),
            )
        };

        match result {
            Ok(anno_id) => Response::ok_data(id, json!({ "annotation_id": anno_id })),
            Err(TextRuntimeError::NodeNotFound(n)) => Response::error(
                id,
                "SENTENCE_NOT_FOUND",
                &format!("sentence '{}' not found", n),
            ),
            Err(TextRuntimeError::AnnotationResolutionError(msg)) => {
                Response::error(id, "ANNOTATION_ERROR", &msg)
            }
            Err(e) => Response::error(id, "ANNOTATION_ERROR", &e.to_string()),
        }
    }

    // ── search ──────────────────────────────────────────────────────────────

    async fn handle_search(
        &self,
        id: &str,
        workspace: &str,
        query: String,
        doc_id: Option<String>,
    ) -> Response {
        let handle = match self.registry.get(workspace) {
            Some(h) => h,
            None => {
                return Response::error(
                    id,
                    "WORKSPACE_NOT_FOUND",
                    &format!("workspace '{}' not found", workspace),
                )
            }
        };

        let result = {
            let rt = handle.runtime.lock().await;
            rt.search(&query, doc_id.as_deref())
        };

        match result {
            Ok(hits) => {
                let results: Vec<Value> = hits
                    .into_iter()
                    .map(|h| {
                        json!({
                            "doc_id": h.doc_id,
                            "sentence_uuid": h.uuid,
                            "node_type": h.node_type,
                            "snippet": h.snippet,
                            "score": h.score,
                        })
                    })
                    .collect();
                Response::ok_data(id, json!({ "results": results }))
            }
            Err(e) => Response::error(id, "SEARCH_ERROR", &e.to_string()),
        }
    }

    // ── toc ─────────────────────────────────────────────────────────────────

    async fn handle_toc(&self, id: &str, workspace: &str, doc_id: String) -> Response {
        let handle = match self.registry.get(workspace) {
            Some(h) => h,
            None => {
                return Response::error(
                    id,
                    "WORKSPACE_NOT_FOUND",
                    &format!("workspace '{}' not found", workspace),
                )
            }
        };

        let result = {
            let rt = handle.runtime.lock().await;
            rt.toc(&doc_id)
        };

        match result {
            Ok(entries) => {
                let toc: Vec<Value> = entries
                    .into_iter()
                    .map(|e| {
                        json!({
                            "uuid": e.uuid,
                            "heading_level": e.heading_level,
                            "title": e.plain_text,
                            "section_path": e.section_path,
                            "position": e.position,
                        })
                    })
                    .collect();
                Response::ok_data(id, json!({ "toc": toc }))
            }
            Err(TextRuntimeError::DocumentNotFound(d)) => Response::error(
                id,
                "DOCUMENT_NOT_FOUND",
                &format!("document '{}' not found", d),
            ),
            Err(e) => Response::error(id, "TOC_ERROR", &e.to_string()),
        }
    }

    // ── status ──────────────────────────────────────────────────────────────

    fn handle_status(&self, id: &str) -> Response {
        let uptime_secs = self.start_time.elapsed().as_secs();
        Response::ok_data(
            id,
            json!({
                "status": "running",
                "version": self.version,
                "uptime_seconds": uptime_secs,
                "workspaces": self.registry.len(),
            }),
        )
    }

    // ── shutdown ────────────────────────────────────────────────────────────

    fn handle_shutdown(&self, id: &str) -> Response {
        tracing::info!("shutdown requested via IPC");
        self.shutdown_token.cancel();
        Response::ok_data(id, json!({ "message": "shutting down" }))
    }
}
