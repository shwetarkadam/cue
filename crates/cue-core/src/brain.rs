use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing::info;

use crate::config::Config;

// ── Data types ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BrainFolder {
    pub id: i64,
    pub name: String,
    pub linked_prompt: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct BrainDocument {
    pub id: i64,
    pub folder_id: i64,
    pub name: String,
    pub content: String,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct BrainNote {
    pub id: i64,
    pub category: String,
    pub content: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct CustomPrompt {
    pub name: String,
    pub description: String,
    pub content: String,
}

// ── Brain store ──────────────────────────────────────────────────────────────

pub struct BrainStore {
    conn: Arc<Mutex<Connection>>,
}

impl BrainStore {
    pub fn new(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)
            .with_context(|| format!("Failed to open brain database: {}", db_path.display()))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS brain_folders (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                linked_prompt TEXT,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS brain_documents (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                folder_id INTEGER NOT NULL REFERENCES brain_folders(id) ON DELETE CASCADE,
                name TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS brain_notes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                category TEXT NOT NULL UNIQUE,
                content TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            "#,
        )
        .context("Failed to create brain schema")?;
        Ok(())
    }

    // ── Folders ──────────────────────────────────────────────────────────

    pub fn create_folder(&self, name: &str, linked_prompt: Option<&str>) -> Result<BrainFolder> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO brain_folders (name, linked_prompt, created_at) VALUES (?1, ?2, ?3)",
            params![name, linked_prompt, now],
        )?;
        let id = conn.last_insert_rowid();
        info!(folder = name, "Brain folder created");
        Ok(BrainFolder {
            id,
            name: name.to_string(),
            linked_prompt: linked_prompt.map(|s| s.to_string()),
            created_at: now,
        })
    }

    pub fn list_folders(&self) -> Result<Vec<BrainFolder>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, linked_prompt, created_at FROM brain_folders ORDER BY name",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(BrainFolder {
                id: row.get(0)?,
                name: row.get(1)?,
                linked_prompt: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn get_folder_by_name(&self, name: &str) -> Result<Option<BrainFolder>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, linked_prompt, created_at FROM brain_folders WHERE name = ?1",
        )?;
        let mut rows = stmt.query_map(params![name], |row| {
            Ok(BrainFolder {
                id: row.get(0)?,
                name: row.get(1)?,
                linked_prompt: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        Ok(rows.next().transpose()?)
    }

    pub fn remove_folder(&self, name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        // Delete documents first (CASCADE should handle it but be explicit)
        if let Ok(folder) = self.get_folder_by_name_inner(&conn, name) {
            if let Some(f) = folder {
                conn.execute("DELETE FROM brain_documents WHERE folder_id = ?1", params![f.id])?;
            }
        }
        let affected = conn.execute("DELETE FROM brain_folders WHERE name = ?1", params![name])?;
        if affected == 0 {
            anyhow::bail!("Folder '{}' not found", name);
        }
        info!(folder = name, "Brain folder removed");
        Ok(())
    }

    fn get_folder_by_name_inner(
        &self,
        conn: &Connection,
        name: &str,
    ) -> Result<Option<BrainFolder>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, linked_prompt, created_at FROM brain_folders WHERE name = ?1",
        )?;
        let mut rows = stmt.query_map(params![name], |row| {
            Ok(BrainFolder {
                id: row.get(0)?,
                name: row.get(1)?,
                linked_prompt: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        Ok(rows.next().transpose()?)
    }

    // ── Documents ────────────────────────────────────────────────────────

    pub fn add_document(&self, folder_name: &str, file_path: &Path) -> Result<BrainDocument> {
        let name = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let content = std::fs::read_to_string(file_path)
            .with_context(|| format!("Failed to read file: {}", file_path.display()))?;

        self.add_document_content(folder_name, &name, &content)
    }

    pub fn add_document_content(
        &self,
        folder_name: &str,
        name: &str,
        content: &str,
    ) -> Result<BrainDocument> {
        let conn = self.conn.lock().unwrap();
        let folder = self
            .get_folder_by_name_inner(&conn, folder_name)?
            .ok_or_else(|| anyhow::anyhow!("Folder '{}' not found", folder_name))?;

        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO brain_documents (folder_id, name, content, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![folder.id, name, content, now],
        )?;
        let id = conn.last_insert_rowid();
        info!(folder = folder_name, doc = name, "Brain document added");
        Ok(BrainDocument {
            id,
            folder_id: folder.id,
            name: name.to_string(),
            content: content.to_string(),
            created_at: now,
        })
    }

    pub fn list_documents(&self, folder_name: &str) -> Result<Vec<BrainDocument>> {
        let conn = self.conn.lock().unwrap();
        let folder = self
            .get_folder_by_name_inner(&conn, folder_name)?
            .ok_or_else(|| anyhow::anyhow!("Folder '{}' not found", folder_name))?;

        let mut stmt = conn.prepare(
            "SELECT id, folder_id, name, content, created_at FROM brain_documents WHERE folder_id = ?1 ORDER BY name",
        )?;
        let rows = stmt.query_map(params![folder.id], |row| {
            Ok(BrainDocument {
                id: row.get(0)?,
                folder_id: row.get(1)?,
                name: row.get(2)?,
                content: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn remove_document(&self, folder_name: &str, doc_name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let folder = self
            .get_folder_by_name_inner(&conn, folder_name)?
            .ok_or_else(|| anyhow::anyhow!("Folder '{}' not found", folder_name))?;

        let affected = conn.execute(
            "DELETE FROM brain_documents WHERE folder_id = ?1 AND name = ?2",
            params![folder.id, doc_name],
        )?;
        if affected == 0 {
            anyhow::bail!("Document '{}' not found in folder '{}'", doc_name, folder_name);
        }
        info!(folder = folder_name, doc = doc_name, "Brain document removed");
        Ok(())
    }

    /// Get all documents for a given prompt name (via linked folder)
    pub fn get_context_for_prompt(&self, prompt_name: &str) -> Result<Vec<BrainDocument>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT bd.id, bd.folder_id, bd.name, bd.content, bd.created_at
            FROM brain_documents bd
            JOIN brain_folders bf ON bf.id = bd.folder_id
            WHERE bf.linked_prompt = ?1
            ORDER BY bd.name
            "#,
        )?;
        let rows = stmt.query_map(params![prompt_name], |row| {
            Ok(BrainDocument {
                id: row.get(0)?,
                folder_id: row.get(1)?,
                name: row.get(2)?,
                content: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    // ── Notes ────────────────────────────────────────────────────────────

    pub fn set_note(&self, category: &str, content: &str) -> Result<BrainNote> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            r#"
            INSERT INTO brain_notes (category, content, updated_at) VALUES (?1, ?2, ?3)
            ON CONFLICT(category) DO UPDATE SET content = ?2, updated_at = ?3
            "#,
            params![category, content, now],
        )?;
        let id = conn.last_insert_rowid();
        info!(category = category, "Brain note saved");
        Ok(BrainNote {
            id,
            category: category.to_string(),
            content: content.to_string(),
            updated_at: now,
        })
    }

    pub fn get_note(&self, category: &str) -> Result<Option<BrainNote>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, category, content, updated_at FROM brain_notes WHERE category = ?1",
        )?;
        let mut rows = stmt.query_map(params![category], |row| {
            Ok(BrainNote {
                id: row.get(0)?,
                category: row.get(1)?,
                content: row.get(2)?,
                updated_at: row.get(3)?,
            })
        })?;
        Ok(rows.next().transpose()?)
    }

    pub fn list_notes(&self) -> Result<Vec<BrainNote>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, category, content, updated_at FROM brain_notes ORDER BY category",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(BrainNote {
                id: row.get(0)?,
                category: row.get(1)?,
                content: row.get(2)?,
                updated_at: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn remove_note(&self, category: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let affected =
            conn.execute("DELETE FROM brain_notes WHERE category = ?1", params![category])?;
        if affected == 0 {
            anyhow::bail!("Note for category '{}' not found", category);
        }
        info!(category = category, "Brain note removed");
        Ok(())
    }

    /// Get note content for a prompt category (used during context assembly)
    pub fn get_note_for_prompt(&self, prompt_name: &str) -> Result<Option<String>> {
        Ok(self.get_note(prompt_name)?.map(|n| n.content))
    }
}

// ── Custom prompts from disk ─────────────────────────────────────────────────

/// Load custom prompts from ~/.config/cue/prompts/
pub fn load_custom_prompts() -> Result<Vec<CustomPrompt>> {
    let prompts_dir = Config::config_dir().join("prompts");
    if !prompts_dir.exists() {
        return Ok(vec![]);
    }

    let mut prompts = Vec::new();
    for entry in std::fs::read_dir(&prompts_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            if let Ok(p) = load_prompt_file(&path) {
                prompts.push(p);
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            if let Ok(p) = load_prompt_markdown(&path) {
                prompts.push(p);
            }
        }
    }
    Ok(prompts)
}

/// Load a TOML prompt file
fn load_prompt_file(path: &Path) -> Result<CustomPrompt> {
    let content = std::fs::read_to_string(path)?;
    let parsed: toml::Value = toml::from_str(&content)?;

    let name = parsed
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("custom")
        })
        .to_string();

    let description = parsed
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("Custom prompt")
        .to_string();

    let prompt_content = parsed
        .get("content")
        .or_else(|| parsed.get("prompt"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Prompt file missing 'content' field: {}", path.display()))?
        .to_string();

    Ok(CustomPrompt {
        name,
        description,
        content: prompt_content,
    })
}

/// Load a markdown file as a prompt (filename = name, content = prompt)
fn load_prompt_markdown(path: &Path) -> Result<CustomPrompt> {
    let content = std::fs::read_to_string(path)?;
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("custom")
        .to_string();

    Ok(CustomPrompt {
        name,
        description: "Custom prompt".to_string(),
        content,
    })
}

/// Get the prompts directory path
pub fn prompts_dir() -> PathBuf {
    Config::config_dir().join("prompts")
}

/// Ensure the prompts directory exists
pub fn ensure_prompts_dir() -> Result<()> {
    std::fs::create_dir_all(prompts_dir())?;
    Ok(())
}
