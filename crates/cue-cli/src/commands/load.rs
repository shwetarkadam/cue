use anyhow::Result;
use clap::Args;
use cue_core::{config::Config, kb::KnowledgeBase};
use std::path::PathBuf;

#[derive(Args, Debug)]
pub struct LoadArgs {
    /// File to ingest into the knowledge base
    pub file: PathBuf,
}

pub fn run(args: LoadArgs) -> Result<()> {
    Config::ensure_dirs()?;
    let db_path = Config::db_path();
    let kb = KnowledgeBase::new(&db_path)?;
    kb.init_schema()?;

    if !args.file.exists() {
        anyhow::bail!("File not found: {}", args.file.display());
    }

    println!("Ingesting {}...", args.file.display());
    let meta = kb.ingest_file(&args.file)?;

    println!(
        "Ingested: {} ({}) — {} chunks",
        meta.name, meta.doc_type, meta.chunk_count
    );
    Ok(())
}
