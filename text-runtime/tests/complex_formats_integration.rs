mod common;

use std::fs;
use std::path::PathBuf;
use text_runtime::{error::TextRuntimeError, runtime::Runtime};

#[tokio::test]
async fn test_ingest_all_formats() -> Result<(), TextRuntimeError> {
    let workspace = PathBuf::from(".test_complex_formats_workspace_async");
    if workspace.exists() {
        fs::remove_dir_all(&workspace).unwrap();
    }
    fs::create_dir_all(&workspace).unwrap();

    let runtime_dir = workspace.join(".textruntime");
    fs::create_dir_all(&runtime_dir).unwrap();
    fs::write(
        runtime_dir.join("config.json"),
        format!("{{\"pandoc_port\": {}}}\n", common::free_port()),
    )
    .unwrap();
    let mut runtime = Runtime::open(&runtime_dir).await?;

    let fixtures_dir = PathBuf::from("tests/fixtures");

    // List of expected fixtures
    let files = vec![
        "complex.html",
        "complex.tex",
        "complex.rst",
        "complex.org",
        "complex.adoc",
        "complex.textile",
        "complex.opml",
        "complex.dbk",
        "complex.jira",
        "complex.wiki",
        "complex.docx",
        "complex.epub",
        "nightmare.md",
    ];

    for file_name in files {
        let file_path = fixtures_dir.join(file_name);
        println!("Testing ingestion of: {:?}", file_path);

        if !file_path.exists() {
            println!("  -> Skipping (File not generated)");
            continue;
        }

        let doc_uuid = match runtime.ingest_file(&file_path).await {
            Ok(uuid) => uuid,
            Err(e) => {
                panic!("Failed to ingest {}: {:?}", file_name, e);
            }
        };

        let nodes = runtime.store.db.get_nodes_by_doc(&doc_uuid)?;
        assert!(
            !nodes.is_empty(),
            "Document {} should have nodes",
            file_name
        );
        println!("  -> Success: {} nodes extracted", nodes.len());
    }

    // Cleanup
    runtime.close().await?;
    fs::remove_dir_all(&workspace).unwrap();
    Ok(())
}

#[tokio::test]
async fn test_project_all_formats() -> Result<(), TextRuntimeError> {
    let workspace = PathBuf::from(".test_complex_formats_projection_async");
    if workspace.exists() {
        fs::remove_dir_all(&workspace).unwrap();
    }
    fs::create_dir_all(&workspace).unwrap();

    let runtime_dir = workspace.join(".textruntime");
    fs::create_dir_all(&runtime_dir).unwrap();
    fs::write(
        runtime_dir.join("config.json"),
        format!("{{\"pandoc_port\": {}}}\n", common::free_port()),
    )
    .unwrap();
    let mut runtime = Runtime::open(&runtime_dir).await?;

    let fixtures_dir = PathBuf::from("tests/fixtures");

    // List of expected fixtures
    let files = vec![
        "complex.html",
        "complex.tex",
        "complex.rst",
        "complex.docx",
        "nightmare.md",
    ];

    for file_name in files {
        let file_path = fixtures_dir.join(file_name);
        println!("Testing projection for: {:?}", file_path);

        if !file_path.exists() {
            println!("  -> Skipping (File not generated)");
            continue;
        }

        // 1. Ingest
        let doc_uuid = runtime.ingest_file(&file_path).await?;

        // 2. Project back to Markdown
        let projected_md = text_runtime::projection::project_document(
            &runtime.store.db,
            &runtime.store.content,
            &runtime.store.config,
            &doc_uuid,
            "markdown",
            false,
        )?;
        assert!(
            !projected_md.text.is_empty(),
            "Projected markdown should not be empty for {}",
            file_name
        );

        // 3. Project back to HTML
        let projected_html = text_runtime::projection::project_document(
            &runtime.store.db,
            &runtime.store.content,
            &runtime.store.config,
            &doc_uuid,
            "html",
            false,
        )?;
        assert!(
            !projected_html.text.is_empty(),
            "Projected html should not be empty for {}",
            file_name
        );

        println!(
            "  -> Successfully projected to Markdown ({} bytes) and HTML ({} bytes)",
            projected_md.text.len(),
            projected_html.text.len()
        );
    }

    // Cleanup
    runtime.close().await?;
    fs::remove_dir_all(&workspace).unwrap();
    Ok(())
}
