// ── Text Runtime CLI ───────────────────────────────────────────────────────
//
// Command-line interface for the Text Runtime. Supports:
//   - ingest: Import a file into the store
//   - read: Project a document to an output format
//   - annotate: Create an annotation
//   - search: Full-text search across documents
//   - daemon: Watch directories and auto-ingest changes

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use text_runtime::runtime::{IngestMetadata, Runtime};

/// Local-first text runtime — ingest, structure, annotate, project.
#[derive(Parser)]
#[command(name = "text-runtime")]
#[command(version = "0.1.0")]
#[command(about = "Local-first text runtime for structured document management")]
struct Cli {
    /// Runtime directory (.textruntime/)
    #[arg(short, long, default_value = ".textruntime")]
    runtime_dir: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Ingest a file into the store.
    Ingest {
        /// Path to the file to ingest.
        path: PathBuf,

        /// Explicit format (auto-detected from extension if not provided).
        #[arg(short, long)]
        format: Option<String>,

        /// Document title (defaults to filename).
        #[arg(short, long)]
        title: Option<String>,
    },

    /// Read (project) a document from the store.
    Read {
        /// Document UUID.
        doc_id: String,

        /// Output format (markdown, html, plain, etc.).
        #[arg(short, long, default_value = "markdown")]
        format: String,

        /// Inject §N sentence markers.
        #[arg(short, long)]
        markers: bool,
    },

    /// Annotate a sentence in a document.
    Annotate {
        /// Document UUID.
        doc_id: String,

        /// UUID of the target sentence (from the marker_map returned by `read --markers`).
        #[arg(long)]
        sentence_uuid: String,

        /// Exact quote text for the annotation target.
        #[arg(short, long)]
        quote: Option<String>,

        /// Annotation body text.
        #[arg(short, long)]
        body: Option<String>,

        /// Annotation motivation (default: "commenting").
        #[arg(short, long)]
        motivation: Option<String>,
    },

    /// Full-text search across all documents.
    Search {
        /// Search query.
        query: String,

        /// Optional: scope search to a specific document.
        #[arg(short, long)]
        doc_id: Option<String>,
    },

    /// List all documents in the store.
    List,

    /// List sentence nodes of a document (uuid, index, text).
    Sentences {
        /// Document UUID.
        doc_id: String,
    },

    /// Start the text-runtime daemon (Unix socket server).
    Daemon {
        /// Path to config file (default: ~/.config/text-runtime/config.toml).
        #[arg(short, long)]
        config: Option<std::path::PathBuf>,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // Open the runtime
    let mut runtime = match Runtime::open(&cli.runtime_dir).await {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!(
                "error: failed to open runtime at {}: {}",
                cli.runtime_dir.display(),
                e
            );
            std::process::exit(1);
        }
    };

    let result = match cli.command {
        Commands::Ingest {
            path,
            format,
            title,
        } => cmd_ingest(&mut runtime, &path, format.as_deref(), title.as_deref()).await,
        Commands::Read {
            doc_id,
            format,
            markers,
        } => cmd_read(&runtime, &doc_id, &format, markers),
        Commands::Annotate {
            doc_id,
            sentence_uuid,
            quote,
            body,
            motivation,
        } => cmd_annotate(
            &runtime,
            &doc_id,
            &sentence_uuid,
            quote.as_deref(),
            body.as_deref(),
            motivation.as_deref(),
        ),
        Commands::Search { query, doc_id } => cmd_search(&runtime, &query, doc_id.as_deref()),
        Commands::List => cmd_list(&runtime),
        Commands::Sentences { doc_id } => cmd_sentences(&runtime, &doc_id),
        Commands::Daemon { config } => {
            // Daemon manages its own Runtime instances — drop the one we opened
            drop(runtime);
            cmd_daemon(config).await
        }
    };

    if let Err(e) = result {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}

async fn cmd_ingest(
    runtime: &mut Runtime,
    path: &PathBuf,
    format: Option<&str>,
    title: Option<&str>,
) -> Result<(), text_runtime::error::TextRuntimeError> {
    if let Some(fmt) = format {
        let text = std::fs::read_to_string(path)
            .map_err(|e| text_runtime::error::TextRuntimeError::io(path, e))?;
        let metadata = IngestMetadata {
            title: title.map(|s| s.to_string()),
            source_path: Some(path.to_string_lossy().to_string()),
            language: None,
        };
        let doc_id = runtime.ingest_text(&text, fmt, &metadata).await?;
        println!("ingested: {}", doc_id);
    } else {
        let doc_id = runtime.ingest_file(path).await?;
        println!("ingested: {}", doc_id);
    }
    Ok(())
}

fn cmd_read(
    runtime: &Runtime,
    doc_id: &str,
    format: &str,
    markers: bool,
) -> Result<(), text_runtime::error::TextRuntimeError> {
    let projection = runtime.read(doc_id, format, markers)?;
    println!("{}", projection.text);
    if let Some(map) = projection.marker_map {
        println!("\n--- markers ---");
        for (k, v) in &map {
            println!("  §{} → {}", k, v);
        }
    }
    Ok(())
}

fn cmd_annotate(
    runtime: &Runtime,
    doc_id: &str,
    sentence_uuid: &str,
    quote: Option<&str>,
    body: Option<&str>,
    motivation: Option<&str>,
) -> Result<(), text_runtime::error::TextRuntimeError> {
    let anno_id = runtime.annotate_by_uuid(doc_id, sentence_uuid, quote, body, motivation)?;
    println!("annotation: {}", anno_id);
    Ok(())
}

fn cmd_search(
    runtime: &Runtime,
    query: &str,
    doc_id: Option<&str>,
) -> Result<(), text_runtime::error::TextRuntimeError> {
    let hits = runtime.search(query, doc_id)?;
    if hits.is_empty() {
        println!("no results found");
    } else {
        for (i, hit) in hits.iter().enumerate() {
            println!(
                "{}. [{}] {} (score: {:.4})",
                i + 1,
                hit.node_type,
                hit.snippet,
                hit.score
            );
            println!("   doc: {}  node: {}", hit.doc_id, hit.uuid);
        }
    }
    Ok(())
}

fn cmd_list(runtime: &Runtime) -> Result<(), text_runtime::error::TextRuntimeError> {
    let docs = runtime.store.db.list_documents()?;
    if docs.is_empty() {
        println!("no documents in store");
        return Ok(());
    }
    for doc in &docs {
        println!(
            "{} | {} | {} | {} | v{}",
            doc.uuid, doc.title, doc.import_format, doc.ingested_at, doc.version
        );
    }
    Ok(())
}

fn cmd_sentences(
    runtime: &Runtime,
    doc_id: &str,
) -> Result<(), text_runtime::error::TextRuntimeError> {
    let nodes = runtime.store.db.get_nodes_by_doc(doc_id)?;
    let sentences: Vec<_> = nodes.iter().filter(|n| n.node_type == "sentence").collect();
    if sentences.is_empty() {
        println!("no sentences found for document {}", doc_id);
        return Ok(());
    }
    // Position order = document order; index 1-based for §N parity.
    for (idx, s) in sentences.iter().enumerate() {
        println!("{} | {} | {}", idx + 1, s.uuid, s.plain_text);
    }
    Ok(())
}

async fn cmd_daemon(
    config_path: Option<std::path::PathBuf>,
) -> Result<(), text_runtime::error::TextRuntimeError> {
    let config = text_runtime::daemon::config::load_config(config_path.as_deref())?;
    println!(
        "starting text-runtime daemon (socket: {})",
        config.socket_path.display()
    );
    text_runtime::daemon::run(config).await
}
