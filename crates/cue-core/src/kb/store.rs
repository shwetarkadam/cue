use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{Connection, params};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing::{debug, info};

use super::chunk::chunk_text;

/// Metadata about an ingested document
#[derive(Debug, Clone)]
pub struct DocumentMeta {
    pub id: i64,
    pub name: String,
    pub path: Option<String>,
    pub doc_type: String,
    pub chunk_count: usize,
    pub created_at: chrono::DateTime<Utc>,
}

/// A retrieved chunk from KB search
#[derive(Debug, Clone)]
pub struct RetrievedChunk {
    pub content: String,
    pub doc_name: String,
    pub score: f32,
}

/// Knowledge base backed by SQLite FTS5
pub struct KnowledgeBase {
    conn: Arc<Mutex<Connection>>,
}

impl KnowledgeBase {
    /// Open or create the knowledge base at the given path
    pub fn new(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)
            .with_context(|| format!("Failed to open KB database: {}", db_path.display()))?;

        // Enable WAL mode for better concurrent access
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;

        let kb = Self {
            conn: Arc::new(Mutex::new(conn)),
        };

        Ok(kb)
    }

    /// Initialize database schema
    pub fn init_schema(&self) -> Result<()> {
        // We need a synchronous version for initialization
        let conn = self.conn.lock().unwrap();
        Self::create_schema(&conn)
    }

    fn create_schema(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS documents (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                path TEXT,
                doc_type TEXT NOT NULL,
                chunk_count INTEGER DEFAULT 0,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS chunks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                doc_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
                content TEXT NOT NULL,
                chunk_index INTEGER NOT NULL
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
                content,
                content='chunks',
                content_rowid='id'
            );

            CREATE TRIGGER IF NOT EXISTS chunks_ai AFTER INSERT ON chunks BEGIN
                INSERT INTO chunks_fts(rowid, content) VALUES (new.id, new.content);
            END;

            CREATE TRIGGER IF NOT EXISTS chunks_ad AFTER DELETE ON chunks BEGIN
                INSERT INTO chunks_fts(chunks_fts, rowid, content) VALUES('delete', old.id, old.content);
            END;

            CREATE TRIGGER IF NOT EXISTS chunks_au AFTER UPDATE ON chunks BEGIN
                INSERT INTO chunks_fts(chunks_fts, rowid, content) VALUES('delete', old.id, old.content);
                INSERT INTO chunks_fts(rowid, content) VALUES (new.id, new.content);
            END;
            "#,
        )
        .context("Failed to create KB schema")?;
        Ok(())
    }

    /// Ingest a file into the knowledge base
    pub fn ingest_file(&self, path: &Path) -> Result<DocumentMeta> {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        let doc_type = match ext.as_str() {
            "pdf" => "pdf",
            "md" | "markdown" => "markdown",
            "txt" => "text",
            "rs" => "rust",
            "py" => "python",
            "js" | "ts" => "javascript",
            _ => "text",
        };

        info!(path = %path.display(), doc_type = %doc_type, "Ingesting document");

        let text = extract_text(path, doc_type)?;
        if text.trim().is_empty() {
            anyhow::bail!("Document is empty or could not be parsed: {}", path.display());
        }

        let chunks = chunk_text(&text, 512, 64);
        let chunk_count = chunks.len();

        info!(chunks = chunk_count, "Document chunked");

        let conn = self.conn.lock().unwrap();
        let created_at = Utc::now().to_rfc3339();

        conn.execute(
            "INSERT INTO documents (name, path, doc_type, chunk_count, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                name,
                path.to_str(),
                doc_type,
                chunk_count as i64,
                created_at
            ],
        )?;

        let doc_id = conn.last_insert_rowid();

        for (i, chunk) in chunks.iter().enumerate() {
            conn.execute(
                "INSERT INTO chunks (doc_id, content, chunk_index) VALUES (?1, ?2, ?3)",
                params![doc_id, chunk, i as i64],
            )?;
        }

        debug!(doc_id = doc_id, "Document ingested");

        Ok(DocumentMeta {
            id: doc_id,
            name,
            path: path.to_str().map(|s| s.to_string()),
            doc_type: doc_type.to_string(),
            chunk_count,
            created_at: Utc::now(),
        })
    }

    /// Search the knowledge base using FTS5
    pub fn search(&self, query: &str, top_k: usize) -> Result<Vec<RetrievedChunk>> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }

        let conn = self.conn.lock().unwrap();

        // Escape FTS5 special characters
        let safe_query = escape_fts5_query(query);

        let mut stmt = conn.prepare(
            r#"
            SELECT c.content, d.name, chunks_fts.rank
            FROM chunks_fts
            JOIN chunks c ON c.id = chunks_fts.rowid
            JOIN documents d ON d.id = c.doc_id
            WHERE chunks_fts MATCH ?1
            ORDER BY chunks_fts.rank
            LIMIT ?2
            "#,
        )?;

        let results = stmt.query_map(params![safe_query, top_k as i64], |row| {
            Ok(RetrievedChunk {
                content: row.get(0)?,
                doc_name: row.get(1)?,
                score: row.get::<_, f64>(2).unwrap_or(0.0) as f32,
            })
        })?;

        let chunks: Result<Vec<_>, _> = results.collect();
        Ok(chunks.context("Failed to collect search results")?)
    }

    /// List all documents in the knowledge base
    pub fn list_documents(&self) -> Result<Vec<DocumentMeta>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, path, doc_type, chunk_count, created_at FROM documents ORDER BY id DESC",
        )?;

        let docs = stmt.query_map([], |row| {
            let created_at_str: String = row.get(5)?;
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)? as usize,
                created_at_str,
            ))
        })?;

        let mut result = Vec::new();
        for doc in docs {
            let (id, name, path, doc_type, chunk_count, created_at_str) = doc?;
            let created_at = chrono::DateTime::parse_from_rfc3339(&created_at_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            result.push(DocumentMeta {
                id,
                name,
                path,
                doc_type,
                chunk_count,
                created_at,
            });
        }

        Ok(result)
    }

    /// Remove a document and its chunks
    pub fn remove_document(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let affected = conn.execute("DELETE FROM documents WHERE id = ?1", params![id])?;
        if affected == 0 {
            anyhow::bail!("Document with id {} not found", id);
        }
        info!(id = id, "Document removed");
        Ok(())
    }
}

/// Extract text from a file based on its type
fn extract_text(path: &Path, doc_type: &str) -> Result<String> {
    match doc_type {
        "pdf" => {
            use lopdf::Document;
            let doc = Document::load(path)
                .with_context(|| format!("Failed to load PDF: {}", path.display()))?;

            let mut text = String::new();
            let mut pages: Vec<u32> = doc.get_pages().keys().cloned().collect();
            pages.sort_unstable();

            for page_num in pages {
                if let Ok(content) = doc.extract_text(&[page_num]) {
                    text.push_str(&content);
                    text.push('\n');
                }
            }

            if text.trim().is_empty() {
                anyhow::bail!("PDF contains no extractable text (may be image-only)");
            }

            Ok(text)
        }
        _ => {
            // Read as text
            std::fs::read_to_string(path)
                .with_context(|| format!("Failed to read file: {}", path.display()))
        }
    }
}

/// Escape special FTS5 characters in query
fn escape_fts5_query(query: &str) -> String {
    // FTS5 special chars: " * ^ ( ) OR AND NOT
    // Simple approach: wrap each word in quotes
    let words: Vec<String> = query
        .split_whitespace()
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{}\"", w.replace('"', "")))
        .collect();

    if words.is_empty() {
        return String::new();
    }

    words.join(" OR ")
}
