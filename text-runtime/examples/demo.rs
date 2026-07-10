use std::fs;
use std::path::PathBuf;
use text_runtime::{error::TextRuntimeError, runtime::Runtime};

#[tokio::main]
async fn main() -> Result<(), TextRuntimeError> {
    println!("🚀 Starting Text-Runtime AI Agent Integration Demo...\n");

    // 1. Initialize the Runtime
    let workspace = PathBuf::from(".demo_workspace");
    if workspace.exists() {
        fs::remove_dir_all(&workspace).unwrap();
    }
    fs::create_dir_all(&workspace).unwrap();

    let runtime_dir = workspace.join(".textruntime");
    let mut runtime = Runtime::open(&runtime_dir).await?;

    // 2. Initial Ingestion
    println!("📝 Agent generates initial draft...");
    let draft_path = workspace.join("draft.md");
    let initial_text = "# Introduction\n\nAI agents need robust text infrastructure to collaborate with humans. This is a crucial capability.";
    fs::write(&draft_path, initial_text).unwrap();

    let doc_id = runtime.ingest_file(&draft_path).await?;
    println!("✅ Ingested successfully. Document ID: {}\n", doc_id);

    // 3. FTS5 Search
    println!("🔍 Agent searches memory for 'collaborate'...");
    let search_results = runtime.search("collaborate", None)?;
    for result in &search_results {
        println!("  -> Found in Node [{}]: {}", result.uuid, result.snippet);
    }
    println!();

    // 4. Modifying and Re-ingesting
    println!("✍️  Agent modifies the file (fixing a typo and adding a paragraph)...");
    let updated_text = "# Introduction\n\nAI agents need robust text infrastructure to collaborate with humans. This is a CRITICAL capability.\n\nWe also need to track node versions.";
    fs::write(&draft_path, updated_text).unwrap();

    // Re-ingest (will diff and merge automatically)
    let _ = runtime.ingest_file(&draft_path).await?;
    println!("✅ Re-ingested successfully with diffing.\n");

    // 5. Query Active Nodes
    println!("📊 Current Active Nodes in AST:");
    let active_nodes = runtime.store.db.get_nodes_by_doc(&doc_id)?;
    for node in active_nodes {
        println!(
            "  - [{}] (v{}) => {}",
            node.node_type, node.version, node.plain_text
        );
    }
    println!();

    // 6. Demonstrate deleted/ghosted nodes are clean
    println!("👻 Querying FTS5 for old 'crucial' token...");
    let search_crucial = runtime.search("crucial", None)?;
    if search_crucial.is_empty() {
        println!("  -> No results! FTS5 index was perfectly synchronized.\n");
    }

    // 7. Creating an Annotation
    println!("📌 Agent creates an annotation on the text...");
    // Retrieve a node to annotate (e.g., the newly added paragraph)
    let target_node = runtime
        .store
        .db
        .get_nodes_by_doc(&doc_id)?
        .into_iter()
        .find(|n| n.plain_text.contains("track node versions"))
        .unwrap();

    let anno_json = format!(
        r#"{{"@context": "http://www.w3.org/ns/anno.jsonld", "type": "Annotation", "body": "Agent thought: We must implement WAL mode", "target": "{}"}}"#,
        target_node.uuid
    );

    let conn = rusqlite::Connection::open(workspace.join(".textruntime/db.sqlite")).unwrap();
    conn.execute(
        "INSERT INTO annotations (uuid, annotation, target_uuid, target_doc_id, motivation, status, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 'commenting', 'active', 'now', 'now')",
        rusqlite::params!["anno-demo-1", anno_json, target_node.uuid, doc_id],
    ).unwrap();

    let annos = runtime.store.db.get_annotations_for_doc(&doc_id)?;
    println!(
        "✅ Retrieved {} active annotations for document.",
        annos.len()
    );
    for a in annos {
        println!("  -> Target UUID: {}", a.target_uuid);
        println!("  -> Status: {}", a.status);
    }

    // Clean up
    fs::remove_dir_all(&workspace).unwrap();
    println!("\n🎉 Demo completed successfully!");

    Ok(())
}
