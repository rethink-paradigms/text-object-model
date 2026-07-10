// ── SQLite Database Store ───────────────────────────────────────────────────
//
// The main SQLite store: schema creation, migrations, all CRUD operations,
// FTS5 full-text search, transaction support.

use std::path::Path;

use rusqlite::{params, Connection, Transaction};

use crate::error::TextRuntimeError;
use crate::store::types::{ActivityRow, AnnotationRow, DocumentRow, NodeRow, TransclusionRow};

/// A row returned by FTS5 search with BM25 ranking.
#[derive(Debug, Clone)]
pub struct SearchResultRow {
    pub uuid: String,
    pub node_type: String,
    pub doc_id: String,
    /// Highlighted snippet with `<mark>...</mark>` tags.
    pub snippet: String,
    /// BM25 score (lower = better match).
    pub score: f64,
}

/// Sanitize a user-provided FTS5 query string.
///
/// Wraps the query in double quotes and escapes internal double quotes.
/// This prevents FTS5 syntax errors from special characters (`*`, `AND`,
/// `OR`, `NEAR`, etc.) in raw user input.
pub fn sanitize_fts_query(query: &str) -> String {
    let escaped = query.replace('"', "\"\"");
    format!("\"{}\"", escaped)
}

/// The SQLite database store.
///
/// Owns a `rusqlite::Connection`. Handles schema initialization
/// (tables, indexes, FTS5, triggers, pragmas) and all CRUD operations
/// for documents, nodes, annotations, transclusions, and activities.
pub struct DbStore {
    conn: Connection,
}

impl DbStore {
    /// Open (or create) the SQLite database at the given path.
    ///
    /// Creates the database file if it doesn't exist, then initializes
    /// the schema (tables, indexes, FTS5 virtual table, sync triggers,
    /// pragmas).
    pub fn open(db_path: &Path) -> Result<Self, TextRuntimeError> {
        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| TextRuntimeError::io(parent, e))?;
        }

        let conn = Connection::open(db_path)?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    /// Initialize schema: CREATE TABLE, CREATE VIRTUAL TABLE nodes_fts,
    /// CREATE sync triggers, set pragmas.
    fn init_schema(&self) -> Result<(), TextRuntimeError> {
        self.conn.execute_batch(
            "
            -- ── Pragmas ────────────────────────────────────────────────
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;
            PRAGMA synchronous = NORMAL;
            PRAGMA temp_store = MEMORY;
            PRAGMA mmap_size = 30000000000;
            PRAGMA page_size = 4096;

            -- ── Core: Documents ────────────────────────────────────────
            CREATE TABLE IF NOT EXISTS documents (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                uuid            TEXT NOT NULL UNIQUE,
                title           TEXT NOT NULL DEFAULT '',
                import_format   TEXT NOT NULL,
                import_path     TEXT,
                import_hash     TEXT,
                root_node_uuid  TEXT,
                version         INTEGER NOT NULL DEFAULT 1,
                ingested_at     TEXT NOT NULL,
                language        TEXT DEFAULT 'en'
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_documents_uuid ON documents(uuid);

            -- ── Core: Nodes ────────────────────────────────────────────
            CREATE TABLE IF NOT EXISTS nodes (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                uuid            TEXT NOT NULL UNIQUE,
                doc_id          TEXT NOT NULL REFERENCES documents(uuid),
                node_type       TEXT NOT NULL,
                parent_uuid     TEXT REFERENCES nodes(uuid),
                position        REAL NOT NULL,
                has_content     INTEGER NOT NULL DEFAULT 0,
                content_path    TEXT,
                plain_text      TEXT NOT NULL DEFAULT '',
                structural_hash TEXT NOT NULL,
                char_start      INTEGER,
                char_end        INTEGER,
                heading_level   INTEGER,
                section_path    TEXT,
                version         INTEGER NOT NULL DEFAULT 1,
                status          TEXT NOT NULL DEFAULT 'active',
                created_at      TEXT NOT NULL,
                updated_at      TEXT NOT NULL
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_nodes_uuid ON nodes(uuid);
            CREATE INDEX IF NOT EXISTS idx_nodes_doc_id ON nodes(doc_id);
            CREATE INDEX IF NOT EXISTS idx_nodes_parent ON nodes(parent_uuid);
            CREATE INDEX IF NOT EXISTS idx_nodes_position ON nodes(doc_id, position);
            CREATE INDEX IF NOT EXISTS idx_nodes_hash ON nodes(doc_id, structural_hash);
            CREATE INDEX IF NOT EXISTS idx_nodes_type ON nodes(node_type);
            CREATE INDEX IF NOT EXISTS idx_nodes_status ON nodes(status);

            -- ── FTS5: Full-text search ─────────────────────────────────
            CREATE VIRTUAL TABLE IF NOT EXISTS nodes_fts USING fts5(
                uuid        UNINDEXED,
                node_type   UNINDEXED,
                doc_id      UNINDEXED,
                plain_text,
                content=nodes,
                content_rowid=id,
                tokenize='porter unicode61'
            );

            -- FTS5 sync triggers
            -- AFTER INSERT
            CREATE TRIGGER IF NOT EXISTS nodes_ai AFTER INSERT ON nodes BEGIN
                INSERT INTO nodes_fts(rowid, uuid, node_type, doc_id, plain_text)
                VALUES (new.id, new.uuid, new.node_type, new.doc_id, new.plain_text);
            END;

            -- AFTER DELETE
            CREATE TRIGGER IF NOT EXISTS nodes_ad AFTER DELETE ON nodes BEGIN
                INSERT INTO nodes_fts(nodes_fts, rowid, uuid, node_type, doc_id, plain_text)
                VALUES ('delete', old.id, old.uuid, old.node_type, old.doc_id, old.plain_text);
            END;

            -- AFTER UPDATE OF plain_text
            CREATE TRIGGER IF NOT EXISTS nodes_au AFTER UPDATE OF plain_text ON nodes BEGIN
                INSERT INTO nodes_fts(nodes_fts, rowid, uuid, node_type, doc_id, plain_text)
                VALUES ('delete', old.id, old.uuid, old.node_type, old.doc_id, old.plain_text);
                INSERT INTO nodes_fts(rowid, uuid, node_type, doc_id, plain_text)
                VALUES (new.id, new.uuid, new.node_type, new.doc_id, new.plain_text);
            END;

            -- ── W3C Web Annotations ────────────────────────────────────
            CREATE TABLE IF NOT EXISTS annotations (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                uuid            TEXT NOT NULL UNIQUE,
                annotation      TEXT NOT NULL,
                target_uuid     TEXT NOT NULL,
                target_doc_id   TEXT NOT NULL,
                motivation      TEXT NOT NULL DEFAULT 'commenting',
                status          TEXT NOT NULL DEFAULT 'active',
                creator         TEXT,
                created_at      TEXT NOT NULL,
                updated_at      TEXT NOT NULL
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_annotations_uuid ON annotations(uuid);
            CREATE INDEX IF NOT EXISTS idx_annotations_target ON annotations(target_uuid);
            CREATE INDEX IF NOT EXISTS idx_annotations_doc ON annotations(target_doc_id);
            CREATE INDEX IF NOT EXISTS idx_annotations_status ON annotations(status);

            -- ── Transclusion Edges ──────────────────────────────────────
            CREATE TABLE IF NOT EXISTS transclusions (
                id                  INTEGER PRIMARY KEY AUTOINCREMENT,
                uuid                TEXT NOT NULL UNIQUE,
                predicate           TEXT NOT NULL,
                source_node_uuid    TEXT NOT NULL REFERENCES nodes(uuid),
                source_doc_uuid     TEXT NOT NULL REFERENCES documents(uuid),
                target_node_uuid    TEXT NOT NULL REFERENCES nodes(uuid),
                target_doc_uuid     TEXT NOT NULL REFERENCES documents(uuid),
                version_at_include  INTEGER NOT NULL,
                status              TEXT NOT NULL DEFAULT 'live',
                created_at          TEXT NOT NULL
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_transclusions_uuid ON transclusions(uuid);
            CREATE INDEX IF NOT EXISTS idx_transclusions_source ON transclusions(source_node_uuid);
            CREATE INDEX IF NOT EXISTS idx_transclusions_target ON transclusions(target_node_uuid);
            CREATE INDEX IF NOT EXISTS idx_transclusions_status ON transclusions(status);

            -- ── Provenance Activities ──────────────────────────────────
            CREATE TABLE IF NOT EXISTS activities (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                uuid            TEXT NOT NULL UNIQUE,
                activity_type   TEXT NOT NULL,
                input_ids       TEXT,
                output_ids      TEXT,
                agent           TEXT,
                config          TEXT,
                started_at      TEXT NOT NULL,
                ended_at        TEXT
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_activities_uuid ON activities(uuid);
            CREATE INDEX IF NOT EXISTS idx_activities_type ON activities(activity_type);
            ",
        )?;

        Ok(())
    }

    // ── Document CRUD ──────────────────────────────────────────────────────

    /// Insert a document row.
    pub fn insert_document(&self, doc: &DocumentRow) -> Result<(), TextRuntimeError> {
        self.conn.execute(
            "INSERT INTO documents (uuid, title, import_format, import_path, import_hash,
             root_node_uuid, version, ingested_at, language)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                doc.uuid,
                doc.title,
                doc.import_format,
                doc.import_path,
                doc.import_hash,
                doc.root_node_uuid,
                doc.version,
                doc.ingested_at,
                doc.language,
            ],
        )?;
        Ok(())
    }

    /// Get a document by UUID.
    pub fn get_document(&self, uuid: &str) -> Result<DocumentRow, TextRuntimeError> {
        self.conn
            .query_row(
                "SELECT id, uuid, title, import_format, import_path, import_hash,
                 root_node_uuid, version, ingested_at, language
                 FROM documents WHERE uuid = ?1",
                params![uuid],
                |row| {
                    Ok(DocumentRow {
                        id: row.get(0)?,
                        uuid: row.get(1)?,
                        title: row.get(2)?,
                        import_format: row.get(3)?,
                        import_path: row.get(4)?,
                        import_hash: row.get(5)?,
                        root_node_uuid: row.get(6)?,
                        version: row.get(7)?,
                        ingested_at: row.get(8)?,
                        language: row.get(9)?,
                    })
                },
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    TextRuntimeError::DocumentNotFound(uuid.to_string())
                }
                other => TextRuntimeError::DatabaseError(other),
            })
    }

    /// Get a document by its import path (provenance lookup).
    pub fn get_document_by_path(
        &self,
        path: &str,
    ) -> Result<Option<DocumentRow>, TextRuntimeError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, uuid, title, import_format, import_path, import_hash,
             root_node_uuid, version, ingested_at, language
             FROM documents WHERE import_path = ?1",
        )?;

        let result = stmt.query_row(params![path], |row| {
            Ok(DocumentRow {
                id: row.get(0)?,
                uuid: row.get(1)?,
                title: row.get(2)?,
                import_format: row.get(3)?,
                import_path: row.get(4)?,
                import_hash: row.get(5)?,
                root_node_uuid: row.get(6)?,
                version: row.get(7)?,
                ingested_at: row.get(8)?,
                language: row.get(9)?,
            })
        });

        match result {
            Ok(doc) => Ok(Some(doc)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(TextRuntimeError::DatabaseError(e)),
        }
    }

    /// List all documents in the store.
    pub fn list_documents(&self) -> Result<Vec<DocumentRow>, TextRuntimeError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, uuid, title, import_format, import_path, import_hash,
             root_node_uuid, version, ingested_at, language
             FROM documents ORDER BY ingested_at DESC",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(DocumentRow {
                id: row.get(0)?,
                uuid: row.get(1)?,
                title: row.get(2)?,
                import_format: row.get(3)?,
                import_path: row.get(4)?,
                import_hash: row.get(5)?,
                root_node_uuid: row.get(6)?,
                version: row.get(7)?,
                ingested_at: row.get(8)?,
                language: row.get(9)?,
            })
        })?;

        let mut docs = Vec::new();
        for row in rows {
            docs.push(row?);
        }
        Ok(docs)
    }

    /// Update a document's version counter.
    pub fn update_document_version(
        &self,
        uuid: &str,
        version: i32,
    ) -> Result<(), TextRuntimeError> {
        let affected = self.conn.execute(
            "UPDATE documents SET version = ?1 WHERE uuid = ?2",
            params![version, uuid],
        )?;
        if affected == 0 {
            return Err(TextRuntimeError::DocumentNotFound(uuid.to_string()));
        }
        Ok(())
    }

    // ── Node CRUD ──────────────────────────────────────────────────────────

    /// Insert a single node row.
    pub fn insert_node(&self, node: &NodeRow) -> Result<(), TextRuntimeError> {
        self.conn.execute(
            "INSERT INTO nodes (uuid, doc_id, node_type, parent_uuid, position,
             has_content, content_path, plain_text, structural_hash,
             char_start, char_end, heading_level, section_path,
             version, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
             ON CONFLICT(uuid) DO UPDATE SET
                 doc_id=excluded.doc_id,
                 node_type=excluded.node_type,
                 parent_uuid=excluded.parent_uuid,
                 position=excluded.position,
                 has_content=excluded.has_content,
                 content_path=excluded.content_path,
                 plain_text=excluded.plain_text,
                 structural_hash=excluded.structural_hash,
                 char_start=excluded.char_start,
                 char_end=excluded.char_end,
                 heading_level=excluded.heading_level,
                 section_path=excluded.section_path,
                 version=excluded.version,
                 status=excluded.status,
                 updated_at=excluded.updated_at",
            params![
                node.uuid,
                node.doc_id,
                node.node_type,
                node.parent_uuid,
                node.position,
                node.has_content as i32,
                node.content_path,
                node.plain_text,
                node.structural_hash,
                node.char_start,
                node.char_end,
                node.heading_level,
                node.section_path,
                node.version,
                node.status,
                node.created_at,
                node.updated_at,
            ],
        )?;
        Ok(())
    }

    /// Insert multiple node rows in a batch (single INSERT with multiple VALUES).
    ///
    /// This is more efficient than calling `insert_node` in a loop,
    /// especially within a transaction.
    pub fn insert_nodes(&self, nodes: &[NodeRow]) -> Result<(), TextRuntimeError> {
        if nodes.is_empty() {
            return Ok(());
        }

        // Build a multi-row INSERT
        let mut sql = String::from(
            "INSERT INTO nodes (uuid, doc_id, node_type, parent_uuid, position,
             has_content, content_path, plain_text, structural_hash,
             char_start, char_end, heading_level, section_path,
             version, status, created_at, updated_at) VALUES ",
        );

        let placeholders: Vec<String> = (0..nodes.len())
            .map(|i| {
                let base = i * 17;
                format!(
                    "(?{},?{},?{},?{},?{},?{},?{},?{},?{},?{},?{},?{},?{},?{},?{},?{},?{})",
                    base + 1,
                    base + 2,
                    base + 3,
                    base + 4,
                    base + 5,
                    base + 6,
                    base + 7,
                    base + 8,
                    base + 9,
                    base + 10,
                    base + 11,
                    base + 12,
                    base + 13,
                    base + 14,
                    base + 15,
                    base + 16,
                    base + 17,
                )
            })
            .collect();

        sql.push_str(&placeholders.join(", "));

        sql.push_str(
            " ON CONFLICT(uuid) DO UPDATE SET
                 doc_id=excluded.doc_id,
                 node_type=excluded.node_type,
                 parent_uuid=excluded.parent_uuid,
                 position=excluded.position,
                 has_content=excluded.has_content,
                 content_path=excluded.content_path,
                 plain_text=excluded.plain_text,
                 structural_hash=excluded.structural_hash,
                 char_start=excluded.char_start,
                 char_end=excluded.char_end,
                 heading_level=excluded.heading_level,
                 section_path=excluded.section_path,
                 version=excluded.version,
                 status=excluded.status,
                 updated_at=excluded.updated_at",
        );

        let mut stmt = self.conn.prepare(&sql)?;

        // Collect all params into a flat Vec<&dyn rusqlite::types::ToSql>
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        for node in nodes {
            param_values.push(Box::new(node.uuid.clone()));
            param_values.push(Box::new(node.doc_id.clone()));
            param_values.push(Box::new(node.node_type.clone()));
            param_values.push(Box::new(node.parent_uuid.clone()));
            param_values.push(Box::new(node.position));
            param_values.push(Box::new(node.has_content as i32));
            param_values.push(Box::new(node.content_path.clone()));
            param_values.push(Box::new(node.plain_text.clone()));
            param_values.push(Box::new(node.structural_hash.clone()));
            param_values.push(Box::new(node.char_start));
            param_values.push(Box::new(node.char_end));
            param_values.push(Box::new(node.heading_level));
            param_values.push(Box::new(node.section_path.clone()));
            param_values.push(Box::new(node.version));
            param_values.push(Box::new(node.status.clone()));
            param_values.push(Box::new(node.created_at.clone()));
            param_values.push(Box::new(node.updated_at.clone()));
        }

        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|b| b.as_ref()).collect();

        stmt.execute(params_ref.as_slice())?;
        Ok(())
    }

    /// Get a node by UUID.
    pub fn get_node(&self, uuid: &str) -> Result<NodeRow, TextRuntimeError> {
        self.conn
            .query_row(
                "SELECT id, uuid, doc_id, node_type, parent_uuid, position,
                 has_content, content_path, plain_text, structural_hash,
                 char_start, char_end, heading_level, section_path,
                 version, status, created_at, updated_at
                 FROM nodes WHERE uuid = ?1",
                params![uuid],
                |row| Ok(Self::row_to_node(row)),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    TextRuntimeError::NodeNotFound(uuid.to_string())
                }
                other => TextRuntimeError::DatabaseError(other),
            })
    }

    /// Get all nodes for a document.
    pub fn get_nodes_by_doc(&self, doc_id: &str) -> Result<Vec<NodeRow>, TextRuntimeError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, uuid, doc_id, node_type, parent_uuid, position,
             has_content, content_path, plain_text, structural_hash,
             char_start, char_end, heading_level, section_path,
             version, status, created_at, updated_at
             FROM nodes WHERE doc_id = ?1 AND status = 'active'
             ORDER BY position",
        )?;

        let rows = stmt.query_map(params![doc_id], |row| Ok(Self::row_to_node(row)))?;
        let mut nodes = Vec::new();
        for row in rows {
            nodes.push(row?);
        }
        Ok(nodes)
    }

    /// Get child nodes for a parent UUID.
    pub fn get_children(&self, parent_uuid: &str) -> Result<Vec<NodeRow>, TextRuntimeError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, uuid, doc_id, node_type, parent_uuid, position,
             has_content, content_path, plain_text, structural_hash,
             char_start, char_end, heading_level, section_path,
             version, status, created_at, updated_at
             FROM nodes WHERE parent_uuid = ?1 AND status = 'active'
             ORDER BY position",
        )?;

        let rows = stmt.query_map(params![parent_uuid], |row| Ok(Self::row_to_node(row)))?;
        let mut nodes = Vec::new();
        for row in rows {
            nodes.push(row?);
        }
        Ok(nodes)
    }

    /// Get nodes by structural hash (for re-ingestion diffing).
    pub fn get_nodes_by_hash(
        &self,
        doc_id: &str,
        hash: &str,
    ) -> Result<Vec<NodeRow>, TextRuntimeError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, uuid, doc_id, node_type, parent_uuid, position,
             has_content, content_path, plain_text, structural_hash,
             char_start, char_end, heading_level, section_path,
             version, status, created_at, updated_at
             FROM nodes WHERE doc_id = ?1 AND structural_hash = ?2 AND status = 'active'",
        )?;

        let rows = stmt.query_map(params![doc_id, hash], |row| Ok(Self::row_to_node(row)))?;
        let mut nodes = Vec::new();
        for row in rows {
            nodes.push(row?);
        }
        Ok(nodes)
    }

    /// Update a node's status (e.g., "deleted").
    pub fn update_node_status(&self, uuid: &str, status: &str) -> Result<(), TextRuntimeError> {
        let affected = self.conn.execute(
            "UPDATE nodes SET status = ?1, updated_at = ?2 WHERE uuid = ?3",
            params![status, chrono::Utc::now().to_rfc3339(), uuid],
        )?;
        if affected == 0 {
            return Err(TextRuntimeError::NodeNotFound(uuid.to_string()));
        }
        Ok(())
    }

    /// Update a node's content: plain_text, structural_hash, and version.
    pub fn update_node_content(
        &self,
        uuid: &str,
        plain_text: &str,
        hash: &str,
        version: i32,
    ) -> Result<(), TextRuntimeError> {
        let affected = self.conn.execute(
            "UPDATE nodes SET plain_text = ?1, structural_hash = ?2, version = ?3,
             updated_at = ?4 WHERE uuid = ?5",
            params![
                plain_text,
                hash,
                version,
                chrono::Utc::now().to_rfc3339(),
                uuid
            ],
        )?;
        if affected == 0 {
            return Err(TextRuntimeError::NodeNotFound(uuid.to_string()));
        }
        Ok(())
    }

    /// Mark multiple nodes as deleted.
    pub fn mark_nodes_deleted(&self, uuids: &[String]) -> Result<(), TextRuntimeError> {
        if uuids.is_empty() {
            return Ok(());
        }

        let now = chrono::Utc::now().to_rfc3339();
        for uuid in uuids {
            self.conn.execute(
                "UPDATE nodes SET status = 'deleted', updated_at = ?1 WHERE uuid = ?2",
                params![now, uuid],
            )?;
        }
        Ok(())
    }

    // ── Annotation CRUD ─────────────────────────────────────────────────────

    /// Insert an annotation row.
    pub fn insert_annotation(&self, anno: &AnnotationRow) -> Result<(), TextRuntimeError> {
        self.conn.execute(
            "INSERT INTO annotations (uuid, annotation, target_uuid, target_doc_id,
             motivation, status, creator, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                anno.uuid,
                anno.annotation,
                anno.target_uuid,
                anno.target_doc_id,
                anno.motivation,
                anno.status,
                anno.creator,
                anno.created_at,
                anno.updated_at,
            ],
        )?;
        Ok(())
    }

    /// Get annotations targeting a specific node UUID.
    pub fn get_annotations_for_target(
        &self,
        target_uuid: &str,
    ) -> Result<Vec<AnnotationRow>, TextRuntimeError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, uuid, annotation, target_uuid, target_doc_id,
             motivation, status, creator, created_at, updated_at
             FROM annotations WHERE target_uuid = ?1 AND status != 'deleted'",
        )?;

        let rows = stmt.query_map(params![target_uuid], |row| Ok(Self::row_to_annotation(row)))?;
        let mut annos = Vec::new();
        for row in rows {
            annos.push(row?);
        }
        Ok(annos)
    }

    /// Get all annotations for a document.
    pub fn get_annotations_for_doc(
        &self,
        doc_id: &str,
    ) -> Result<Vec<AnnotationRow>, TextRuntimeError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, uuid, annotation, target_uuid, target_doc_id,
             motivation, status, creator, created_at, updated_at
             FROM annotations WHERE target_doc_id = ?1 AND status != 'deleted'",
        )?;

        let rows = stmt.query_map(params![doc_id], |row| Ok(Self::row_to_annotation(row)))?;
        let mut annos = Vec::new();
        for row in rows {
            annos.push(row?);
        }
        Ok(annos)
    }

    /// Update an annotation's status.
    pub fn update_annotation_status(
        &self,
        uuid: &str,
        status: &str,
    ) -> Result<(), TextRuntimeError> {
        self.conn.execute(
            "UPDATE annotations SET status = ?1, updated_at = ?2 WHERE uuid = ?3",
            params![status, chrono::Utc::now().to_rfc3339(), uuid],
        )?;
        Ok(())
    }

    /// Mark annotations targeting deleted nodes as "orphan".
    pub fn update_orphaned_annotations(
        &self,
        node_uuids: &[String],
    ) -> Result<(), TextRuntimeError> {
        if node_uuids.is_empty() {
            return Ok(());
        }

        let now = chrono::Utc::now().to_rfc3339();
        for uuid in node_uuids {
            self.conn.execute(
                "UPDATE annotations SET status = 'orphan', updated_at = ?1
                 WHERE target_uuid = ?2 AND status = 'active'",
                params![now, uuid],
            )?;
        }
        Ok(())
    }

    // ── FTS5 Search ────────────────────────────────────────────────────────

    /// Search FTS5 with BM25 ranking.
    ///
    /// Returns matching node UUIDs with highlighted snippets.
    /// The query is sanitized before execution to prevent FTS5 syntax errors.
    pub fn search_fts(
        &self,
        query: &str,
        doc_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchResultRow>, TextRuntimeError> {
        let sanitized = sanitize_fts_query(query);

        let mut sql = String::from(
            "SELECT n.uuid, n.node_type, n.doc_id,
                    highlight(nodes_fts, 3, '<mark>', '</mark>') AS snippet,
                    bm25(nodes_fts, 0.0, 0.0, 0.0, 1.0) AS score
             FROM nodes_fts
             JOIN nodes n ON nodes_fts.rowid = n.id
             WHERE nodes_fts MATCH ?1
               AND n.status = 'active'",
        );

        if doc_id.is_some() {
            sql.push_str(" AND n.doc_id = ?2");
        }

        sql.push_str(" ORDER BY bm25(nodes_fts) LIMIT ?");

        let mut stmt = self.conn.prepare(&sql)?;

        let rows: Vec<SearchResultRow> = if let Some(doc) = doc_id {
            let limit_param = if limit > 0 { limit as i64 } else { 50 };
            stmt.query_map(params![sanitized, doc, limit_param], |row| {
                Ok(SearchResultRow {
                    uuid: row.get(0)?,
                    node_type: row.get(1)?,
                    doc_id: row.get(2)?,
                    snippet: row.get(3)?,
                    score: row.get(4)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect()
        } else {
            let limit_param = if limit > 0 { limit as i64 } else { 50 };
            stmt.query_map(params![sanitized, limit_param], |row| {
                Ok(SearchResultRow {
                    uuid: row.get(0)?,
                    node_type: row.get(1)?,
                    doc_id: row.get(2)?,
                    snippet: row.get(3)?,
                    score: row.get(4)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect()
        };

        Ok(rows)
    }

    // ── Transclusion CRUD ───────────────────────────────────────────────────

    /// Insert a transclusion edge.
    pub fn insert_transclusion(&self, edge: &TransclusionRow) -> Result<(), TextRuntimeError> {
        self.conn.execute(
            "INSERT INTO transclusions (uuid, predicate, source_node_uuid, source_doc_uuid,
             target_node_uuid, target_doc_uuid, version_at_include, status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                edge.uuid,
                edge.predicate,
                edge.source_node_uuid,
                edge.source_doc_uuid,
                edge.target_node_uuid,
                edge.target_doc_uuid,
                edge.version_at_include,
                edge.status,
                edge.created_at,
            ],
        )?;
        Ok(())
    }

    /// Get transclusion edges where the given node is the source.
    pub fn get_transclusions_for_source(
        &self,
        node_uuid: &str,
    ) -> Result<Vec<TransclusionRow>, TextRuntimeError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, uuid, predicate, source_node_uuid, source_doc_uuid,
             target_node_uuid, target_doc_uuid, version_at_include, status, created_at
             FROM transclusions WHERE source_node_uuid = ?1",
        )?;

        let rows = stmt.query_map(params![node_uuid], |row| Ok(Self::row_to_transclusion(row)))?;
        let mut edges = Vec::new();
        for row in rows {
            edges.push(row?);
        }
        Ok(edges)
    }

    /// Get transclusion edges where the given node is the target.
    pub fn get_transclusions_for_target(
        &self,
        node_uuid: &str,
    ) -> Result<Vec<TransclusionRow>, TextRuntimeError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, uuid, predicate, source_node_uuid, source_doc_uuid,
             target_node_uuid, target_doc_uuid, version_at_include, status, created_at
             FROM transclusions WHERE target_node_uuid = ?1",
        )?;

        let rows = stmt.query_map(params![node_uuid], |row| Ok(Self::row_to_transclusion(row)))?;
        let mut edges = Vec::new();
        for row in rows {
            edges.push(row?);
        }
        Ok(edges)
    }

    /// Detect stale transclusion edges: those where the target node's
    /// version has changed since the edge was created.
    pub fn detect_stale_edges(
        &self,
        doc_id: Option<&str>,
    ) -> Result<Vec<TransclusionRow>, TextRuntimeError> {
        let mut sql = String::from(
            "SELECT t.id, t.uuid, t.predicate, t.source_node_uuid, t.source_doc_uuid,
                    t.target_node_uuid, t.target_doc_uuid, t.version_at_include,
                    t.status, t.created_at
             FROM transclusions t
             JOIN nodes n ON t.target_node_uuid = n.uuid
             WHERE t.version_at_include < n.version
               AND t.status = 'live'",
        );

        let rows: Vec<TransclusionRow> = if let Some(doc) = doc_id {
            sql.push_str(" AND t.source_doc_uuid = ?1");
            let mut stmt = self.conn.prepare(&sql)?;
            let result: Vec<TransclusionRow> = stmt
                .query_map(params![doc], |row| Ok(Self::row_to_transclusion(row)))?
                .filter_map(|r| r.ok())
                .collect();
            result
        } else {
            let mut stmt = self.conn.prepare(&sql)?;
            let result: Vec<TransclusionRow> = stmt
                .query_map([], |row| Ok(Self::row_to_transclusion(row)))?
                .filter_map(|r| r.ok())
                .collect();
            result
        };

        Ok(rows)
    }

    /// Update a transclusion edge's status.
    pub fn update_transclusion_status(
        &self,
        uuid: &str,
        status: &str,
    ) -> Result<(), TextRuntimeError> {
        self.conn.execute(
            "UPDATE transclusions SET status = ?1 WHERE uuid = ?2",
            params![status, uuid],
        )?;
        Ok(())
    }

    // ── Activity CRUD ──────────────────────────────────────────────────────

    /// Insert an activity row (append-only — activities are never updated).
    pub fn insert_activity(&self, activity: &ActivityRow) -> Result<(), TextRuntimeError> {
        self.conn.execute(
            "INSERT INTO activities (uuid, activity_type, input_ids, output_ids,
             agent, config, started_at, ended_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                activity.uuid,
                activity.activity_type,
                activity.input_ids,
                activity.output_ids,
                activity.agent,
                activity.config,
                activity.started_at,
                activity.ended_at,
            ],
        )?;
        Ok(())
    }

    /// Get activities, optionally filtered by document ID and/or activity type.
    pub fn get_activities(
        &self,
        doc_id: Option<&str>,
        activity_type: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<ActivityRow>, TextRuntimeError> {
        let mut sql = String::from(
            "SELECT id, uuid, activity_type, input_ids, output_ids,
             agent, config, started_at, ended_at
             FROM activities WHERE 1=1",
        );

        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(doc) = doc_id {
            sql.push_str(" AND (input_ids LIKE ? OR output_ids LIKE ?)");
            let pattern = format!("%{}%", doc);
            param_values.push(Box::new(pattern.clone()));
            param_values.push(Box::new(pattern));
        }

        if let Some(at) = activity_type {
            let idx = param_values.len() + 1;
            sql.push_str(&format!(" AND activity_type = ?{}", idx));
            param_values.push(Box::new(at.to_string()));
        }

        sql.push_str(" ORDER BY started_at DESC");

        if let Some(lim) = limit {
            let idx = param_values.len() + 1;
            sql.push_str(&format!(" LIMIT ?{}", idx));
            param_values.push(Box::new(lim as i64));
        }

        let mut stmt = self.conn.prepare(&sql)?;

        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|b| b.as_ref()).collect();

        let rows = stmt.query_map(params_ref.as_slice(), |row| {
            Ok(ActivityRow {
                id: row.get(0)?,
                uuid: row.get(1)?,
                activity_type: row.get(2)?,
                input_ids: row.get(3)?,
                output_ids: row.get(4)?,
                agent: row.get(5)?,
                config: row.get(6)?,
                started_at: row.get(7)?,
                ended_at: row.get(8)?,
            })
        })?;

        let mut activities = Vec::new();
        for row in rows {
            activities.push(row?);
        }
        Ok(activities)
    }

    // ── Transaction ─────────────────────────────────────────────────────────

    /// Execute a closure within a transaction.
    ///
    /// If the closure returns `Ok`, the transaction is committed.
    /// If the closure returns `Err`, the transaction is rolled back.
    pub fn transaction<T, F>(&mut self, f: F) -> Result<T, TextRuntimeError>
    where
        F: FnOnce(&Transaction) -> Result<T, TextRuntimeError>,
    {
        let tx = self.conn.transaction()?;
        let result = f(&tx);
        match result {
            Ok(val) => {
                tx.commit()?;
                Ok(val)
            }
            Err(e) => {
                // Rollback happens automatically on drop, but explicit is clearer
                let _ = tx.rollback();
                Err(e)
            }
        }
    }

    /// Close the database connection, consuming the store.
    pub fn close(self) -> Result<(), TextRuntimeError> {
        // Connection is closed when dropped; explicit close for clarity.
        // rusqlite::Connection::close() was added in 0.31.
        // If not available, we rely on Drop.
        drop(self.conn);
        Ok(())
    }

    // ── Private helpers ─────────────────────────────────────────────────────

    fn row_to_node(row: &rusqlite::Row) -> NodeRow {
        NodeRow {
            id: row.get(0).unwrap_or(0),
            uuid: row.get(1).unwrap_or_default(),
            doc_id: row.get(2).unwrap_or_default(),
            node_type: row.get(3).unwrap_or_default(),
            parent_uuid: row.get(4).unwrap_or(None),
            position: row.get(5).unwrap_or(0.0),
            has_content: row.get::<_, i32>(6).unwrap_or(0) != 0,
            content_path: row.get(7).unwrap_or(None),
            plain_text: row.get(8).unwrap_or_default(),
            structural_hash: row.get(9).unwrap_or_default(),
            char_start: row.get(10).unwrap_or(None),
            char_end: row.get(11).unwrap_or(None),
            heading_level: row.get(12).unwrap_or(None),
            section_path: row.get(13).unwrap_or(None),
            version: row.get(14).unwrap_or(1),
            status: row.get(15).unwrap_or_else(|_| "active".to_string()),
            created_at: row.get(16).unwrap_or_default(),
            updated_at: row.get(17).unwrap_or_default(),
        }
    }

    fn row_to_annotation(row: &rusqlite::Row) -> AnnotationRow {
        AnnotationRow {
            id: row.get(0).unwrap_or(0),
            uuid: row.get(1).unwrap_or_default(),
            annotation: row.get(2).unwrap_or_default(),
            target_uuid: row.get(3).unwrap_or_default(),
            target_doc_id: row.get(4).unwrap_or_default(),
            motivation: row.get(5).unwrap_or_default(),
            status: row.get(6).unwrap_or_default(),
            creator: row.get(7).unwrap_or(None),
            created_at: row.get(8).unwrap_or_default(),
            updated_at: row.get(9).unwrap_or_default(),
        }
    }

    fn row_to_transclusion(row: &rusqlite::Row) -> TransclusionRow {
        TransclusionRow {
            id: row.get(0).unwrap_or(0),
            uuid: row.get(1).unwrap_or_default(),
            predicate: row.get(2).unwrap_or_default(),
            source_node_uuid: row.get(3).unwrap_or_default(),
            source_doc_uuid: row.get(4).unwrap_or_default(),
            target_node_uuid: row.get(5).unwrap_or_default(),
            target_doc_uuid: row.get(6).unwrap_or_default(),
            version_at_include: row.get(7).unwrap_or(0),
            status: row.get(8).unwrap_or_default(),
            created_at: row.get(9).unwrap_or_default(),
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_db() -> (DbStore, TempDir) {
        let tmp = TempDir::new().expect("temp dir");
        let db_path = tmp.path().join("test.sqlite");
        let store = DbStore::open(&db_path).expect("open db");
        (store, tmp)
    }

    fn now() -> String {
        chrono::Utc::now().to_rfc3339()
    }

    #[test]
    fn test_schema_init() {
        let (store, _tmp) = setup_db();

        // Verify tables exist by querying sqlite_master
        let count: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='nodes'",
                [],
                |row| row.get(0),
            )
            .expect("query sqlite_master");
        assert_eq!(count, 1, "nodes table should exist");

        let count: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='documents'",
                [],
                |row| row.get(0),
            )
            .expect("query sqlite_master");
        assert_eq!(count, 1, "documents table should exist");

        // Verify FTS5 virtual table exists
        let count: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='nodes_fts'",
                [],
                |row| row.get(0),
            )
            .expect("query sqlite_master");
        assert_eq!(count, 1, "nodes_fts should exist");

        // Verify triggers exist
        let count: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger'",
                [],
                |row| row.get(0),
            )
            .expect("query sqlite_master");
        assert_eq!(count, 3, "3 FTS5 sync triggers should exist");

        // Verify pragmas
        let journal_mode: String = store
            .conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .expect("pragma journal_mode");
        assert_eq!(journal_mode, "wal");
    }

    #[test]
    fn test_insert_get_node() {
        let (store, _tmp) = setup_db();
        let ts = now();

        // Insert a document first (FK constraint)
        store
            .insert_document(&DocumentRow {
                id: 0,
                uuid: "019f4a1b-2c3d-7e4f-8a5b-6c7d8e9f0001".to_string(),
                title: "Test Doc".to_string(),
                import_format: "markdown".to_string(),
                import_path: None,
                import_hash: None,
                root_node_uuid: None,
                version: 1,
                ingested_at: ts.clone(),
                language: "en".to_string(),
            })
            .expect("insert document");

        let node_uuid = "019f4a1b-2c3d-7e4f-8a5b-6c7d8e9f0a1b".to_string();
        let node = NodeRow {
            id: 0,
            uuid: node_uuid.clone(),
            doc_id: "019f4a1b-2c3d-7e4f-8a5b-6c7d8e9f0001".to_string(),
            node_type: "paragraph".to_string(),
            parent_uuid: None,
            position: 1000.0,
            has_content: true,
            content_path: Some("01/019f4a1b-2c3d-7e4f-8a5b-6c7d8e9f0a1b.json".to_string()),
            plain_text: "Hello world.".to_string(),
            structural_hash: "abc123def456".to_string(),
            char_start: None,
            char_end: None,
            heading_level: None,
            section_path: None,
            version: 1,
            status: "active".to_string(),
            created_at: ts.clone(),
            updated_at: ts.clone(),
        };

        store.insert_node(&node).expect("insert node");

        let retrieved = store.get_node(&node_uuid).expect("get node");
        assert_eq!(retrieved.uuid, node_uuid);
        assert_eq!(retrieved.node_type, "paragraph");
        assert_eq!(retrieved.plain_text, "Hello world.");
        assert_eq!(retrieved.status, "active");
    }

    #[test]
    fn test_fts5_search() {
        let (store, _tmp) = setup_db();
        let ts = now();
        let doc_uuid = "019f4a1b-2c3d-7e4f-8a5b-6c7d8e9f0001".to_string();

        store
            .insert_document(&DocumentRow {
                id: 0,
                uuid: doc_uuid.clone(),
                title: "Search Test".to_string(),
                import_format: "markdown".to_string(),
                import_path: None,
                import_hash: None,
                root_node_uuid: None,
                version: 1,
                ingested_at: ts.clone(),
                language: "en".to_string(),
            })
            .expect("insert document");

        // Insert a node with searchable text
        store
            .insert_node(&NodeRow {
                id: 0,
                uuid: "019f4a1b-2c3d-7e4f-8a5b-000000000001".to_string(),
                doc_id: doc_uuid.clone(),
                node_type: "paragraph".to_string(),
                parent_uuid: None,
                position: 1000.0,
                has_content: true,
                content_path: None,
                plain_text: "The quick brown fox jumps over the lazy dog.".to_string(),
                structural_hash: "hash1".to_string(),
                char_start: None,
                char_end: None,
                heading_level: None,
                section_path: None,
                version: 1,
                status: "active".to_string(),
                created_at: ts.clone(),
                updated_at: ts.clone(),
            })
            .expect("insert node");

        // Insert a second node (different text)
        store
            .insert_node(&NodeRow {
                id: 0,
                uuid: "019f4a1b-2c3d-7e4f-8a5b-000000000002".to_string(),
                doc_id: doc_uuid.clone(),
                node_type: "paragraph".to_string(),
                parent_uuid: None,
                position: 2000.0,
                has_content: true,
                content_path: None,
                plain_text: "A completely different sentence about physics.".to_string(),
                structural_hash: "hash2".to_string(),
                char_start: None,
                char_end: None,
                heading_level: None,
                section_path: None,
                version: 1,
                status: "active".to_string(),
                created_at: ts.clone(),
                updated_at: ts.clone(),
            })
            .expect("insert node");

        // Search for "fox" — should match first node
        let results = store.search_fts("fox", None, 10).expect("search fts");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].uuid, "019f4a1b-2c3d-7e4f-8a5b-000000000001");
        assert!(results[0].snippet.contains("fox"));

        // Search for "physics" — should match second node
        let results = store.search_fts("physics", None, 10).expect("search fts");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].uuid, "019f4a1b-2c3d-7e4f-8a5b-000000000002");

        // Search scoped to doc
        let results = store
            .search_fts("quick", Some(&doc_uuid), 10)
            .expect("search fts");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].doc_id, doc_uuid);
    }

    #[test]
    fn test_sanitize_fts_query() {
        // Normal query
        let sanitized = sanitize_fts_query("hello world");
        assert_eq!(sanitized, "\"hello world\"");

        // Query with special FTS5 characters
        let sanitized = sanitize_fts_query("hello* AND world");
        assert_eq!(sanitized, "\"hello* AND world\"");

        // Query with internal quotes
        let sanitized = sanitize_fts_query("it's \"special\"");
        assert_eq!(sanitized, "\"it's \"\"special\"\"\"");
    }
}
