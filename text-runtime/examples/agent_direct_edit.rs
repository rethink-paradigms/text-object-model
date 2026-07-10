use std::fs;
use std::path::PathBuf;
use text_runtime::{error::TextRuntimeError, runtime::Runtime, types::Uuid};

#[tokio::main]
async fn main() -> Result<(), TextRuntimeError> {
    println!("🤖 Agent is loading the runtime...");
    let workspace = PathBuf::from(".agent_scratch_workspace");
    if workspace.exists() {
        fs::remove_dir_all(&workspace).unwrap();
    }
    fs::create_dir_all(&workspace).unwrap();

    let runtime_dir = workspace.join(".textruntime");
    let mut runtime = Runtime::open(&runtime_dir).await?;

    let input_path = PathBuf::from("/Users/samanvayayagsen/.gemini/antigravity/brain/e502b97a-37c2-4794-ba8f-ed9dedf1861c/scratch/test_input.md");
    println!("📥 Ingesting: {:?}", input_path);
    let doc_uuid = runtime.ingest_file(&input_path).await?;
    println!("✅ Document ingested with UUID: {}", doc_uuid);

    // 1. Retrieve the document's nodes to find paragraph 2
    println!("\n🔍 Finding paragraph 2 in the database...");
    let nodes = runtime.store.db.get_nodes_by_doc(&doc_uuid)?;
    let target_node = nodes
        .iter()
        .find(|n| {
            n.node_type == "paragraph"
                && n.plain_text.contains("update this status update directly")
        })
        .expect("Should find paragraph 2");

    println!("🎯 Found Target Node! UUID: {}", target_node.uuid);
    println!("   Old plain text: \"{}\"", target_node.plain_text);

    // 2. We want to update this paragraph. Instead of string replacing the file,
    // we parse the replacement text to a Pandoc AST and overwrite the content store directly.
    println!("\n✍️ Parsing replacement text to Pandoc AST...");
    let replacement_text = "This is paragraph 2. UPDATED DIRECTLY BY THE AGENT AT THE AST LEVEL! (No regex, no offset errors)";
    let replacement_ast =
        text_runtime::pipeline::parser::parse_to_ast(&runtime.pandoc, replacement_text, "markdown")
            .await?;

    // The result is a full Pandoc document AST. We extract the first block (the Para block)
    let replacement_block = replacement_ast
        .blocks
        .first()
        .expect("Should have at least one block");

    let block_json = serde_json::to_vec_pretty(replacement_block)
        .map_err(TextRuntimeError::SerializationError)?;

    // 3. Update SQLite record and the Content Store using the node's immutable UUID
    println!("💾 Overwriting node content in database and content store...");
    let uuid: Uuid = target_node.uuid.parse().unwrap();
    runtime.store.content.put(&uuid, &block_json)?;

    // We also update the plain text in SQLite so FTS search matches the new text
    let conn = rusqlite::Connection::open(runtime_dir.join("db.sqlite")).unwrap();
    conn.execute(
        "UPDATE nodes SET plain_text = ?1 WHERE uuid = ?2",
        rusqlite::params![replacement_text, target_node.uuid],
    )
    .unwrap();

    // 4. Project the document back to Markdown!
    println!("\n🔄 Projecting document back to Markdown...");
    let projection = text_runtime::projection::project_document(
        &runtime.store.db,
        &runtime.store.content,
        &runtime.store.config,
        &doc_uuid,
        "markdown",
        false,
    )?;

    println!(
        "\n📖 Projected Output:\n--------------------\n{}\n--------------------",
        projection.text
    );

    // 5. Verify the updated document is valid Markdown
    let output_path = PathBuf::from("/Users/samanvayayagsen/.gemini/antigravity/brain/e502b97a-37c2-4794-ba8f-ed9dedf1861c/scratch/test_output.md");
    fs::write(&output_path, &projection.text).unwrap();
    println!("💾 Saved output to: {:?}", output_path);

    // Clean up runtime
    runtime.close().await?;
    fs::remove_dir_all(&workspace).unwrap();

    Ok(())
}
