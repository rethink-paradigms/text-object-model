// ── Text Runtime ──────────────────────────────────────────────────────────
//
// The main Runtime struct — the central API surface. Agents and CLI tools
// interact with the runtime through its public methods. Coordinates:
//   - Store (SQLite database + Content file store + Config)
//   - Pandoc server (PandocManager)
//   - Session-local §N→Uuid marker map

use std::path::{Path, PathBuf};

use crate::cfg::RuntimeConfig;
use crate::error::TextRuntimeError;
use crate::pandoc_mgr::PandocManager;
use crate::pipeline::ingest::{run_pipeline, IngestInput};
use crate::projection::{project_document, Projection};
use crate::store::types::AnnotationRow;
use crate::store::Store;
use crate::transclusion;
use crate::uuid7::uuid7;

/// A search hit returned by FTS5 full-text search.
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub uuid: String,
    pub node_type: String,
    pub doc_id: String,
    pub snippet: String,
    pub score: f64,
}

/// A table-of-contents entry (heading node).
#[derive(Debug, Clone)]
pub struct TocEntry {
    pub uuid: String,
    pub plain_text: String,
    pub heading_level: i32,
    pub section_path: Option<String>,
    pub position: f64,
}

/// Metadata passed to `ingest_text` for provenance tracking.
#[derive(Debug, Clone, Default)]
pub struct IngestMetadata {
    pub title: Option<String>,
    pub source_path: Option<String>,
    pub language: Option<String>,
}

/// The central Runtime struct.
///
/// Composes a Store (DbStore + ContentStore + RuntimeConfig) and a
/// PandocManager for AST parsing/rendering.
///
/// In daemon usage this struct lives behind `tokio::sync::Mutex<Runtime>`
/// inside a `WorkspaceHandle`, which serialises all operations.
pub struct Runtime {
    pub store: Store,
    pub pandoc: PandocManager,
    runtime_dir: PathBuf,
}

impl Runtime {
    /// Open the runtime at the given directory.
    ///
    /// Creates (or opens) the SQLite database, the content file store,
    /// loads/creates the config, and starts the pandoc server.
    ///
    /// The `runtime_dir` is the `.textruntime/` directory within the
    /// user's project root.
    pub async fn open(runtime_dir: &Path) -> Result<Self, TextRuntimeError> {
        let store = Store::open(runtime_dir)?;
        let cfg = store.config.clone();

        let mut pandoc = PandocManager::new(cfg);
        // Start pandoc-server
        pandoc.start().await?;

        Ok(Self {
            store,
            pandoc,
            runtime_dir: runtime_dir.to_path_buf(),
        })
    }

    /// Close the runtime, shutting down the pandoc server and closing the
    /// database connection.
    ///
    /// Note: because `Runtime` implements `Drop`, we cannot move fields out
    /// of `self`. Both `PandocManager` and `Store` implement `Drop` themselves
    /// and will clean up correctly when `self` is dropped at the end of this
    /// function. The explicit `Ok(())` is returned for API compatibility.
    pub async fn close(self) -> Result<(), TextRuntimeError> {
        // self.pandoc (PandocManager) and self.store (Store) both impl Drop.
        // Dropping `self` here triggers both — pandoc kills child process,
        // store closes SQLite connection + WAL checkpoint.
        Ok(())
    }

    // ── Ingest ──────────────────────────────────────────────────────────

    /// Ingest a file from disk.
    ///
    /// Reads the file, detects its format from the extension, and runs
    /// the full ingestion pipeline (parse → segment → assign UUIDs →
    /// write content files → SQLite insert → FTS sync).
    ///
    /// Returns the document UUID.
    pub async fn ingest_file(&mut self, path: &Path) -> Result<String, TextRuntimeError> {
        let source_path = path.to_string_lossy().to_string();
        let title = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("untitled")
            .to_string();

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let is_binary = matches!(ext.as_str(), "docx" | "epub");

        // We declare `text` here so it lives long enough for the Text borrow
        let text = if !is_binary {
            Some(std::fs::read_to_string(path).map_err(|e| TextRuntimeError::io(path, e))?)
        } else {
            None
        };

        let input = if let Some(ref t) = text {
            IngestInput::Text(t.as_str())
        } else {
            IngestInput::BinaryFile(path)
        };

        let result = run_pipeline(
            input,
            None, // auto-detect format from extension
            &title,
            Some(&source_path),
            &mut self.store,
            &self.pandoc,
            true, // merge = true for re-ingest
        )
        .await?;

        Ok(result.document_uuid)
    }

    /// Ingest raw text with explicit format and metadata.
    ///
    /// Runs the full ingestion pipeline without reading from disk.
    ///
    /// Returns the document UUID.
    pub async fn ingest_text(
        &mut self,
        text: &str,
        format: &str,
        metadata: &IngestMetadata,
    ) -> Result<String, TextRuntimeError> {
        let title = metadata.title.as_deref().unwrap_or("untitled");
        let source_path = metadata.source_path.as_deref();

        let result = run_pipeline(
            IngestInput::Text(text),
            Some(format),
            title,
            source_path,
            &mut self.store,
            &self.pandoc,
            false,
        )
        .await?;

        Ok(result.document_uuid)
    }

    // ── Read / Project ──────────────────────────────────────────────────

    /// Read (project) a document from the store to the requested output format.
    ///
    /// If `markers` is true, §N markers are injected at sentence boundaries.
    /// The returned `Projection.marker_map` maps §N numbers to sentence UUIDs
    /// for that specific projection. The caller is responsible for holding the
    /// marker_map and resolving §N → UUID before calling `annotate_by_uuid`.
    ///
    /// Returns a `Projection` with the rendered text, format, and optional
    /// marker map.
    pub fn read(
        &self,
        doc_id: &str,
        format: &str,
        markers: bool,
    ) -> Result<Projection, TextRuntimeError> {
        let projection = project_document(
            &self.store.db,
            &self.store.content,
            &self.store.config,
            doc_id,
            format,
            markers,
        )?;

        Ok(projection)
    }

    // ── Annotate ────────────────────────────────────────────────────────

    /// Create an annotation targeting a sentence node by its UUID.
    ///
    /// `sentence_uuid` is the stable UUID of the target sentence, obtained
    /// from the `marker_map` field of a prior `read()` response.
    /// Callers are responsible for resolving §N → UUID using that map.
    ///
    /// Returns the annotation UUID.
    pub fn annotate_by_uuid(
        &self,
        doc_id: &str,
        sentence_uuid: &str,
        quote: Option<&str>,
        body: Option<&str>,
        motivation: Option<&str>,
    ) -> Result<String, TextRuntimeError> {
        // Get the target node for position information
        let node = self
            .store
            .db
            .get_node(sentence_uuid)
            .map_err(|_| TextRuntimeError::NodeNotFound(sentence_uuid.to_string()))?;

        // Build annotation JSON-LD
        let anno_uuid = uuid7().to_string();
        let now = chrono::Utc::now().to_rfc3339();

        let annotation_json = if let Some(quote_text) = quote {
            // Verify the quote is within the sentence text
            if !node.plain_text.contains(quote_text) {
                return Err(TextRuntimeError::AnnotationResolutionError(format!(
                    "quote '{}' not found within sentence {}",
                    quote_text, sentence_uuid
                )));
            }

            serde_json::json!({
                "@context": "http://www.w3.org/ns/anno.jsonld",
                "type": "Annotation",
                "id": anno_uuid,
                "target": {
                    "source": doc_id,
                    "selector": [
                        {
                            "type": "TextPositionSelector",
                            "start": node.char_start.unwrap_or(0),
                            "end": node.char_end.unwrap_or(0)
                        },
                        {
                            "type": "TextQuoteSelector",
                            "exact": quote_text
                        }
                    ]
                },
                "body": {
                    "type": "TextualBody",
                    "value": body.unwrap_or(""),
                    "purpose": "commenting"
                },
                "motivation": motivation.unwrap_or("commenting"),
                "created": now,
                "modified": now
            })
        } else {
            serde_json::json!({
                "@context": "http://www.w3.org/ns/anno.jsonld",
                "type": "Annotation",
                "id": anno_uuid,
                "target": {
                    "source": doc_id,
                    "selector": [
                        {
                            "type": "TextPositionSelector",
                            "start": node.char_start.unwrap_or(0),
                            "end": node.char_end.unwrap_or(0)
                        }
                    ]
                },
                "body": {
                    "type": "TextualBody",
                    "value": body.unwrap_or(""),
                    "purpose": "commenting"
                },
                "motivation": motivation.unwrap_or("commenting"),
                "created": now,
                "modified": now
            })
        };

        let annotation_str = serde_json::to_string(&annotation_json)?;

        let anno_row = AnnotationRow {
            id: 0,
            uuid: anno_uuid.clone(),
            annotation: annotation_str,
            target_uuid: sentence_uuid.to_string(),
            target_doc_id: doc_id.to_string(),
            motivation: motivation.unwrap_or("commenting").to_string(),
            status: "active".to_string(),
            creator: Some("text-runtime".to_string()),
            created_at: now.clone(),
            updated_at: now,
        };

        self.store.db.insert_annotation(&anno_row)?;

        Ok(anno_uuid)
    }

    // ── Search ──────────────────────────────────────────────────────────

    /// Search all documents using FTS5 full-text search.
    ///
    /// If `doc_id` is provided, the search is scoped to that document.
    /// Returns a list of `SearchHit` with highlighted snippets and BM25 scores.
    pub fn search(
        &self,
        query: &str,
        doc_id: Option<&str>,
    ) -> Result<Vec<SearchHit>, TextRuntimeError> {
        let results = self.store.db.search_fts(query, doc_id, 50)?;

        let hits: Vec<SearchHit> = results
            .into_iter()
            .map(|r| SearchHit {
                uuid: r.uuid,
                node_type: r.node_type,
                doc_id: r.doc_id,
                snippet: r.snippet,
                score: r.score,
            })
            .collect();

        Ok(hits)
    }

    // ── Transclude ──────────────────────────────────────────────────────

    /// Create a transclusion edge between two nodes.
    ///
    /// `source` and `target` are node UUIDs. `predicate` describes the
    /// relationship (e.g., "transcludes", "cites", "derives-from").
    ///
    /// Returns the transclusion edge UUID.
    pub fn transclude(
        &self,
        source: &str,
        target: &str,
        predicate: &str,
    ) -> Result<String, TextRuntimeError> {
        transclusion::create_transclusion(&self.store.db, source, target, predicate)
    }

    // ── Table of Contents ───────────────────────────────────────────────

    /// Get the table of contents for a document (all heading nodes).
    pub fn toc(&self, doc_id: &str) -> Result<Vec<TocEntry>, TextRuntimeError> {
        let headings = crate::store::queries::toc(&self.store.db, doc_id)?;

        let entries: Vec<TocEntry> = headings
            .into_iter()
            .map(|n| TocEntry {
                uuid: n.uuid,
                plain_text: n.plain_text,
                heading_level: n.heading_level.unwrap_or(1),
                section_path: n.section_path,
                position: n.position,
            })
            .collect();

        Ok(entries)
    }

    /// Access the runtime configuration.
    pub fn config(&self) -> &RuntimeConfig {
        &self.store.config
    }

    /// Access the runtime directory path.
    pub fn runtime_dir(&self) -> &Path {
        &self.runtime_dir
    }
}

/// Best-effort synchronous cleanup when Runtime is dropped without explicit close().
///
/// This fires when a WorkspaceHandle is dropped mid-run (e.g., after workspace_remove
/// drains all Arc holders). The pandoc child process is killed via PandocManager's own
/// Drop impl; rusqlite Connection closes on drop (WAL checkpoint runs automatically).
impl Drop for Runtime {
    fn drop(&mut self) {
        // PandocManager::drop handles SIGKILL of pandoc-server child.
        // Store::drop handles rusqlite Connection close + WAL checkpoint.
        // Nothing extra needed here — both are handled by their own Drop impls.
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_open_close() {
        let (_tmp, runtime_dir) = crate::test_util::runtime_dir_with_free_port();

        let rt = tokio::runtime::Runtime::new().expect("tokio");
        rt.block_on(async {
            // Open (may fail if pandoc-server not installed, but DB should work)
            let runtime = Runtime::open(&runtime_dir).await.expect("open runtime");

            // Verify directory structure exists
            assert!(runtime_dir.join("db.sqlite").exists());
            assert!(runtime_dir.join("content").exists());
            assert!(runtime_dir.join("tmp").exists());

            // Close
            runtime.close().await.expect("close runtime");
        });
    }

    #[test]
    fn test_search_empty_store() {
        let (_tmp, runtime_dir) = crate::test_util::runtime_dir_with_free_port();

        let rt = tokio::runtime::Runtime::new().expect("tokio");
        rt.block_on(async {
            let runtime = Runtime::open(&runtime_dir).await.expect("open runtime");

            let results = runtime.search("hello", None).expect("search");
            assert!(results.is_empty());

            runtime.close().await.expect("close");
        });
    }

    #[test]
    fn test_toc_empty_document() {
        let (_tmp, runtime_dir) = crate::test_util::runtime_dir_with_free_port();

        let rt = tokio::runtime::Runtime::new().expect("tokio");
        rt.block_on(async {
            let mut runtime = Runtime::open(&runtime_dir).await.expect("open runtime");

            // Ingest a document with a paragraph but no headings
            let doc_id = runtime
                .ingest_text(
                    "This document has no headings.",
                    "markdown",
                    &Default::default(),
                )
                .await
                .expect("ingest empty");

            // toc on empty document returns empty vec
            let entries = runtime.toc(&doc_id).expect("toc");
            assert!(entries.is_empty());

            // toc on nonexistent document returns error
            let result = runtime.toc("nonexistent");
            assert!(result.is_err());

            runtime.close().await.expect("close");
        });
    }
}
