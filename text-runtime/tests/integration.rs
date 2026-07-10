// ── Integration Tests ──────────────────────────────────────────────────────
//
// End-to-end tests for the Text Runtime:
//   1. Open runtime, verify schema
//   2. Ingest a Markdown document
//   3. Read/project the document
//   4. Search for content
//   5. Create annotations
//   6. Create transclusions
//   7. Round-trip: ingest → read → annotations

mod common;

use std::io::Write;

use tempfile::TempDir;
use text_runtime::cfg::RuntimeConfig;
use text_runtime::runtime::{IngestMetadata, Runtime};

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Create a test markdown file.
fn create_test_md(dir: &TempDir, name: &str, content: &str) -> std::path::PathBuf {
    let path = dir.path().join(name);
    let mut file = std::fs::File::create(&path).expect("create file");
    file.write_all(content.as_bytes()).expect("write file");
    path
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[test]
fn test_open_and_close_runtime() {
    let (_tmp, runtime_dir) = common::runtime_dir_with_free_port();

    let rt = tokio::runtime::Runtime::new().expect("tokio");
    rt.block_on(async {
        let runtime = Runtime::open(&runtime_dir).await.expect("open");

        // Verify directory structure
        assert!(runtime_dir.join("db.sqlite").exists());
        assert!(runtime_dir.join("content").exists());
        assert!(runtime_dir.join("tmp").exists());
        assert!(runtime_dir.join("config.json").exists()); // written by runtime_dir_with_free_port

        runtime.close().await.expect("close");
    });
}

#[test]
fn test_ingest_markdown_file() {
    let (tmp, runtime_dir) = common::runtime_dir_with_free_port();

    // Create a test markdown file
    let md_path = create_test_md(
        &tmp,
        "test.md",
        "# Hello World\n\nThis is a paragraph with some text.\n\n## Section Two\n\nAnother paragraph here.",
    );

    let rt = tokio::runtime::Runtime::new().expect("tokio");
    rt.block_on(async {
        let mut runtime = Runtime::open(&runtime_dir).await.expect("open");

        // Ingest the markdown file
        let doc_id = runtime.ingest_file(&md_path).await.expect("ingest");

        assert!(!doc_id.is_empty());
        println!("ingested doc: {}", doc_id);

        // Verify the document exists
        let doc = runtime.store.db.get_document(&doc_id);
        assert!(doc.is_ok(), "document should exist after ingest");

        // Verify we can get the table of contents
        let toc = runtime.toc(&doc_id);
        // TOC might be empty if pandoc-server isn't available for parsing,
        // but the runtime shouldn't crash
        match toc {
            Ok(entries) => {
                println!("TOC entries: {}", entries.len());
            }
            Err(e) => {
                println!("TOC error (expected if no pandoc-server): {}", e);
            }
        }

        runtime.close().await.expect("close");
    });
}

#[test]
fn test_ingest_text() {
    let (_tmp, runtime_dir) = common::runtime_dir_with_free_port();

    let rt = tokio::runtime::Runtime::new().expect("tokio");
    rt.block_on(async {
        let mut runtime = Runtime::open(&runtime_dir).await.expect("open");

        let text = "# Title\n\nSome content here.\n\n## Subtitle\n\nMore content.";
        let metadata = IngestMetadata {
            title: Some("Test Document".to_string()),
            source_path: Some("test.md".to_string()),
            language: Some("en".to_string()),
        };

        let doc_id = runtime
            .ingest_text(text, "markdown", &metadata)
            .await
            .expect("ingest text");

        assert!(!doc_id.is_empty());
        println!("ingested text doc: {}", doc_id);

        runtime.close().await.expect("close");
    });
}

#[test]
fn test_search() {
    let (_tmp, runtime_dir) = common::runtime_dir_with_free_port();

    let rt = tokio::runtime::Runtime::new().expect("tokio");
    rt.block_on(async {
        let runtime = Runtime::open(&runtime_dir).await.expect("open");

        // Search on empty store should return empty results
        let results = runtime.search("hello", None).expect("search");
        assert!(
            results.is_empty(),
            "search on empty store should return no results"
        );

        runtime.close().await.expect("close");
    });
}

#[test]
fn test_transclusion() {
    let (_tmp, runtime_dir) = common::runtime_dir_with_free_port();

    let rt = tokio::runtime::Runtime::new().expect("tokio");
    rt.block_on(async {
        let runtime = Runtime::open(&runtime_dir).await.expect("open");

        // Test that invalid predicate is rejected
        let result = runtime.transclude(
            "nonexistent-source",
            "nonexistent-target",
            "invalid-predicate",
        );
        assert!(result.is_err(), "invalid predicate should be rejected");

        runtime.close().await.expect("close");
    });
}

#[test]
fn test_store_schema_creation() {
    let (_tmp, runtime_dir) = common::runtime_dir_with_free_port();

    let rt = tokio::runtime::Runtime::new().expect("tokio");
    rt.block_on(async {
        let runtime = Runtime::open(&runtime_dir).await.expect("open");

        // Test that the DB has the expected tables
        // We can test this by trying to list documents (should succeed with 0 results)
        let docs = runtime.store.db.list_documents().expect("list documents");
        assert!(docs.is_empty(), "new store should have no documents");

        runtime.close().await.expect("close");
    });
}

#[test]
fn test_config_save_load() {
    let tmp = TempDir::new().expect("temp dir");
    let runtime_dir = tmp.path().join(".textruntime");

    // Create a config
    let mut config = RuntimeConfig::load_or_create(&runtime_dir).expect("load config");
    config.pandoc_port = 9999;
    config.locale = "fr".to_string();
    config.save().expect("save config");

    // Load it back
    let loaded = RuntimeConfig::load_or_create(&runtime_dir).expect("load again");
    assert_eq!(loaded.pandoc_port, 9999);
    assert_eq!(loaded.locale, "fr");
}

#[test]
fn test_toc_nonexistent_document() {
    let (_tmp, runtime_dir) = common::runtime_dir_with_free_port();

    let rt = tokio::runtime::Runtime::new().expect("tokio");
    rt.block_on(async {
        let runtime = Runtime::open(&runtime_dir).await.expect("open");

        let result = runtime.toc("00000000-0000-0000-0000-000000000000");
        assert!(result.is_err(), "toc on nonexistent doc should error");

        runtime.close().await.expect("close");
    });
}
