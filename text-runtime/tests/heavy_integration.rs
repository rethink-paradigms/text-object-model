mod common;

use std::io::Write;
use std::sync::Arc;
use tempfile::TempDir;
use text_runtime::runtime::Runtime;
use tokio::task;

async fn setup_runtime() -> (Runtime, TempDir) {
    let (tmp, runtime_dir) = common::runtime_dir_with_free_port();
    let runtime = Runtime::open(&runtime_dir).await.expect("open runtime");
    (runtime, tmp)
}

fn create_test_md(dir: &TempDir, name: &str, content: &str) -> std::path::PathBuf {
    let path = dir.path().join(name);
    let mut file = std::fs::File::create(&path).expect("create file");
    file.write_all(content.as_bytes()).expect("write file");
    path
}

#[tokio::test]
async fn test_reingest_hierarchy_shift() {
    let (mut runtime, tmp) = setup_runtime().await;

    let path = create_test_md(&tmp, "shift.md", "## Heading\n\nSome paragraph.\n");
    let doc_id = runtime.ingest_file(&path).await.unwrap();

    let nodes1 = runtime.store.db.get_nodes_by_doc(&doc_id).unwrap();
    let heading1 = nodes1.iter().find(|n| n.node_type == "heading").unwrap();
    let para1 = nodes1.iter().find(|n| n.node_type == "paragraph").unwrap();

    assert_eq!(heading1.heading_level, Some(2));

    // Mutate file to H3
    create_test_md(&tmp, "shift.md", "### Heading\n\nSome paragraph.\n");
    let doc_id2 = runtime.ingest_file(&path).await.unwrap();

    assert_eq!(doc_id, doc_id2);

    let nodes2 = runtime.store.db.get_nodes_by_doc(&doc_id2).unwrap();
    let heading2 = nodes2.iter().find(|n| n.node_type == "heading").unwrap();
    let para2 = nodes2.iter().find(|n| n.node_type == "paragraph").unwrap();

    // Verify UUID carries over
    assert_eq!(
        heading1.uuid, heading2.uuid,
        "UUID should carry over via exact matching"
    );
    assert_eq!(
        para1.uuid, para2.uuid,
        "Paragraph UUID should remain perfectly matched"
    );

    // Verify UPSERT updated the level
    assert_eq!(heading2.heading_level, Some(3));
}

#[tokio::test]
async fn test_reingest_block_reordering() {
    let (mut runtime, tmp) = setup_runtime().await;

    let path = create_test_md(&tmp, "swap.md", "Paragraph A\n\nParagraph B\n");
    let doc_id = runtime.ingest_file(&path).await.unwrap();

    let mut nodes1 = runtime.store.db.get_nodes_by_doc(&doc_id).unwrap();
    nodes1.retain(|n| n.node_type == "paragraph");
    assert_eq!(nodes1.len(), 2);

    let para_a_uuid = nodes1[0].uuid.clone();
    let para_b_uuid = nodes1[1].uuid.clone();

    assert!(
        nodes1[0].position < nodes1[1].position,
        "A should be before B"
    );

    // Mutate file to swap
    create_test_md(&tmp, "swap.md", "Paragraph B\n\nParagraph A\n");
    let _ = runtime.ingest_file(&path).await.unwrap();

    let mut nodes2 = runtime.store.db.get_nodes_by_doc(&doc_id).unwrap();
    nodes2.retain(|n| n.node_type == "paragraph");
    assert_eq!(nodes2.len(), 2);

    let new_b = &nodes2[0];
    let new_a = &nodes2[1];

    assert_eq!(new_b.plain_text, "Paragraph B");
    assert_eq!(new_a.plain_text, "Paragraph A");

    assert_eq!(new_b.uuid, para_b_uuid, "UUID B should carry over");
    assert_eq!(new_a.uuid, para_a_uuid, "UUID A should carry over");

    assert!(new_b.position < new_a.position, "B should now be before A");
}

#[tokio::test]
async fn test_reingest_identical_siblings() {
    let (mut runtime, tmp) = setup_runtime().await;

    let path = create_test_md(&tmp, "twins.md", "Warning!\n\nWarning!\n\nWarning!\n");
    let doc_id = runtime.ingest_file(&path).await.unwrap();

    let mut nodes1 = runtime.store.db.get_nodes_by_doc(&doc_id).unwrap();
    nodes1.retain(|n| n.node_type == "paragraph");
    assert_eq!(nodes1.len(), 3);

    // Mutate file to delete middle warning
    create_test_md(&tmp, "twins.md", "Warning!\n\nWarning!\n");
    let _ = runtime.ingest_file(&path).await.unwrap();

    let mut nodes2 = runtime.store.db.get_nodes_by_doc(&doc_id).unwrap();
    nodes2.retain(|n| n.node_type == "paragraph");

    // The query returns active nodes
    assert_eq!(nodes2.len(), 2);

    let db_path = tmp.path().join(".textruntime").join("db.sqlite");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let mut stmt = conn.prepare("SELECT count(*) FROM nodes WHERE doc_id = ?1 AND status = 'deleted' AND node_type = 'paragraph'").unwrap();
    let deleted_count: usize = stmt
        .query_row(rusqlite::params![doc_id], |r| r.get(0))
        .unwrap();
    assert_eq!(deleted_count, 1);
}

#[tokio::test]
async fn test_fts5_ghost_tokens() {
    let (mut runtime, tmp) = setup_runtime().await;

    let path = create_test_md(&tmp, "ghost.md", "I ate an APPLE today.\n");
    let _ = runtime.ingest_file(&path).await.unwrap();

    let res_apple1 = runtime.search("APPLE", None).unwrap();
    assert!(!res_apple1.is_empty(), "Should find APPLE");

    let res_banana1 = runtime.search("BANANA", None).unwrap();
    assert_eq!(res_banana1.len(), 0);

    // Mutate file
    create_test_md(&tmp, "ghost.md", "I ate an BANANA today.\n");
    let _ = runtime.ingest_file(&path).await.unwrap();

    // APPLE should be gone from index
    let res_apple2 = runtime.search("APPLE", None).unwrap();
    assert_eq!(res_apple2.len(), 0, "Ghost tokens remain in FTS5 index!");

    let res_banana2 = runtime.search("BANANA", None).unwrap();
    assert!(!res_banana2.is_empty(), "Should find BANANA");
}

#[tokio::test]
async fn test_concurrent_read_write_wal_stress() {
    let tmp = TempDir::new().expect("temp dir");
    let runtime_dir = tmp.path().join(".textruntime");

    // Initial setup
    {
        std::fs::create_dir_all(&runtime_dir).unwrap();
        std::fs::write(
            runtime_dir.join("config.json"),
            format!("{{\"pandoc_port\": {}}}\n", common::free_port()),
        )
        .unwrap();
        let mut runtime = Runtime::open(&runtime_dir).await.expect("open runtime");
        let path = create_test_md(&tmp, "stress.md", "Initial content.\n");
        runtime.ingest_file(&path).await.unwrap();
    }

    let dir_path = Arc::new(runtime_dir.clone());
    let tmp_path = Arc::new(tmp.path().to_path_buf());

    // Spawn 1 writer thread
    let writer_dir = Arc::clone(&dir_path);
    let writer_tmp = Arc::clone(&tmp_path);
    let writer_task = task::spawn(async move {
        std::fs::create_dir_all(&*writer_dir).unwrap();
        std::fs::write(
            writer_dir.join("config.json"),
            format!("{{\"pandoc_port\": {}}}\n", common::free_port()),
        )
        .unwrap();
        let mut runtime = Runtime::open(&writer_dir).await.expect("writer open");
        for i in 0..10 {
            let path = writer_tmp.join("stress.md");
            let mut file = std::fs::File::create(&path).expect("create file");
            let content = format!("Updated content loop {}.\n", i);
            file.write_all(content.as_bytes()).expect("write");

            runtime.ingest_file(&path).await.expect("writer ingest");
        }
    });

    // Spawn 5 reader threads
    let mut reader_tasks = vec![];
    for _ in 0..5 {
        let reader_dir = Arc::clone(&dir_path);
        let reader_task = task::spawn(async move {
            std::fs::create_dir_all(&*reader_dir).unwrap();
            std::fs::write(
                reader_dir.join("config.json"),
                format!("{{\"pandoc_port\": {}}}\n", common::free_port()),
            )
            .unwrap();
            let runtime = Runtime::open(&reader_dir).await.expect("reader open");
            for _ in 0..20 {
                let _ = runtime.search("content", None);
                let _ = runtime.store.db.list_documents();
            }
        });
        reader_tasks.push(reader_task);
    }

    writer_task.await.unwrap();
    for r in reader_tasks {
        r.await.unwrap();
    }
}

#[tokio::test]
async fn test_reingest_orphans_annotations() {
    let (mut runtime, tmp) = setup_runtime().await;

    let path = create_test_md(&tmp, "anno.md", "Paragraph 1.\n\nParagraph 2.\n");
    let doc_id = runtime.ingest_file(&path).await.unwrap();

    let nodes = runtime.store.db.get_nodes_by_doc(&doc_id).unwrap();
    println!("NODES:");
    for n in &nodes {
        println!(" - [{}]: {}", n.node_type, n.plain_text);
    }
    let s2 = nodes
        .iter()
        .find(|n| n.plain_text.contains("Paragraph 2") && n.node_type == "sentence")
        .unwrap();

    let db_path = tmp.path().join(".textruntime").join("db.sqlite");
    let conn = rusqlite::Connection::open(&db_path).unwrap();

    let anno_json = format!(
        r#"{{"@context": "http://www.w3.org/ns/anno.jsonld", "id": "anno-1", "target": {{"source": "{}", "selector": {{"type": "TextPositionSelector", "start": 0, "end": 10}}}}}}"#,
        doc_id
    );
    conn.execute(
        "INSERT INTO annotations (uuid, annotation, target_uuid, target_doc_id, motivation, status, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 'commenting', 'active', 'now', 'now')",
        rusqlite::params!["anno-1", anno_json, s2.uuid, doc_id],
    ).unwrap();

    let annos1 = runtime.store.db.get_annotations_for_doc(&doc_id).unwrap();
    assert_eq!(annos1.len(), 1);
    assert_eq!(annos1[0].status, "active");

    // Mutate file: Delete Paragraph 2
    create_test_md(&tmp, "anno.md", "Paragraph 1.\n");
    let _ = runtime.ingest_file(&path).await.unwrap();

    // Annotation should now be orphaned
    let annos2 = runtime.store.db.get_annotations_for_doc(&doc_id).unwrap();
    assert_eq!(annos2.len(), 1);
    assert_eq!(annos2[0].status, "orphan");
}
