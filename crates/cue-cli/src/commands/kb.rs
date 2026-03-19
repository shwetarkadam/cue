use anyhow::Result;
use clap::{Args, Subcommand};
use cue_core::{config::Config, kb::KnowledgeBase};

#[derive(Args, Debug)]
pub struct KbArgs {
    #[command(subcommand)]
    pub command: KbCommands,
}

#[derive(Subcommand, Debug)]
pub enum KbCommands {
    /// List all documents in the knowledge base
    List,
    /// Search the knowledge base
    Search {
        /// Search query
        query: String,
        /// Number of results to return
        #[arg(short, long, default_value = "5")]
        top_k: usize,
    },
    /// Remove a document from the knowledge base
    Remove {
        /// Document ID (from kb list)
        id: i64,
    },
}

pub fn run(args: KbArgs) -> Result<()> {
    Config::ensure_dirs()?;
    let db_path = Config::db_path();
    let kb = KnowledgeBase::new(&db_path)?;
    kb.init_schema()?;

    match args.command {
        KbCommands::List => {
            let docs = kb.list_documents()?;
            if docs.is_empty() {
                println!("Knowledge base is empty.");
                println!("Add documents with: cue load <file>");
            } else {
                println!("{:<6} {:<10} {:<8} {}", "ID", "ADDED", "CHUNKS", "NAME");
                println!("{}", "-".repeat(50));
                for doc in &docs {
                    println!(
                        "{:<6} {:<10} {:<8} {}",
                        doc.id,
                        doc.created_at.format("%Y-%m-%d"),
                        doc.chunk_count,
                        doc.name
                    );
                }
                println!("\n{} documents", docs.len());
            }
        }
        KbCommands::Search { query, top_k } => {
            let chunks = kb.search(&query, top_k)?;
            if chunks.is_empty() {
                println!("No results found for: {}", query);
            } else {
                println!("Found {} results for: {}\n", chunks.len(), query);
                for (i, chunk) in chunks.iter().enumerate() {
                    println!("[{}] From: {}", i + 1, chunk.doc_name);
                    println!("{}", "-".repeat(40));
                    // Print first 200 chars of content
                    let preview = if chunk.content.len() > 200 {
                        format!("{}...", &chunk.content[..200])
                    } else {
                        chunk.content.clone()
                    };
                    println!("{}\n", preview);
                }
            }
        }
        KbCommands::Remove { id } => {
            kb.remove_document(id)?;
            println!("Document {} removed.", id);
        }
    }
    Ok(())
}
