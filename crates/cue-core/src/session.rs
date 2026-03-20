use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing::{debug, info};

#[derive(Debug, Clone)]
pub struct Session {
    pub id: i64,
    pub title: Option<String>,
    pub prompt_template: String,
    pub started_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct TranscriptEntry {
    pub id: Option<i64>,
    pub session_id: i64,
    pub channel: String,
    pub text: String,
    pub spoken_at: DateTime<Utc>,
}

pub struct SessionStore {
    conn: Arc<Mutex<Connection>>,
}

impl SessionStore {
    pub fn new(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)
            .with_context(|| format!("Failed to open session database: {}", db_path.display()))?;

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        Self::create_schema(&conn)
    }

    fn create_schema(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                title TEXT,
                prompt_template TEXT NOT NULL DEFAULT 'general',
                started_at TEXT NOT NULL,
                ended_at TEXT
            );

            CREATE TABLE IF NOT EXISTS transcript (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id INTEGER NOT NULL REFERENCES sessions(id),
                channel TEXT NOT NULL,
                text TEXT NOT NULL,
                spoken_at TEXT NOT NULL
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS transcript_fts USING fts5(
                text,
                content='transcript',
                content_rowid='id'
            );

            CREATE TRIGGER IF NOT EXISTS transcript_ai AFTER INSERT ON transcript BEGIN
                INSERT INTO transcript_fts(rowid, text) VALUES (new.id, new.text);
            END;

            CREATE TRIGGER IF NOT EXISTS transcript_ad AFTER DELETE ON transcript BEGIN
                INSERT INTO transcript_fts(transcript_fts, rowid, text) VALUES('delete', old.id, old.text);
            END;

            CREATE TABLE IF NOT EXISTS exchanges (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id INTEGER NOT NULL REFERENCES sessions(id),
                query TEXT NOT NULL,
                response TEXT NOT NULL,
                provider TEXT NOT NULL,
                model TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL
            );
            "#,
        )
        .context("Failed to create session schema")?;
        Ok(())
    }

    pub fn create_session(&self, prompt_template: &str) -> Result<Session> {
        let conn = self.conn.lock().unwrap();
        let started_at = Utc::now();
        conn.execute(
            "INSERT INTO sessions (prompt_template, started_at) VALUES (?1, ?2)",
            params![prompt_template, started_at.to_rfc3339()],
        )?;
        let id = conn.last_insert_rowid();
        info!(session_id = id, prompt = %prompt_template, "Session created");
        Ok(Session {
            id,
            title: None,
            prompt_template: prompt_template.to_string(),
            started_at,
        })
    }

    pub fn add_transcript(&self, entry: &TranscriptEntry) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO transcript (session_id, channel, text, spoken_at) VALUES (?1, ?2, ?3, ?4)",
            params![
                entry.session_id,
                entry.channel,
                entry.text,
                entry.spoken_at.to_rfc3339()
            ],
        )?;
        let id = conn.last_insert_rowid();
        debug!(id = id, channel = %entry.channel, "Transcript entry added");
        Ok(id)
    }

    pub fn get_recent_transcript(
        &self,
        session_id: i64,
        window_secs: u64,
    ) -> Result<Vec<TranscriptEntry>> {
        let conn = self.conn.lock().unwrap();
        let cutoff = Utc::now() - chrono::Duration::seconds(window_secs as i64);

        let mut stmt = conn.prepare(
            "SELECT id, session_id, channel, text, spoken_at
             FROM transcript
             WHERE session_id = ?1 AND spoken_at >= ?2
             ORDER BY spoken_at ASC",
        )?;

        let entries = stmt.query_map(params![session_id, cutoff.to_rfc3339()], |row| {
            let spoken_at_str: String = row.get(4)?;
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                spoken_at_str,
            ))
        })?;

        let mut result = Vec::new();
        for entry in entries {
            let (id, session_id, channel, text, spoken_at_str) = entry?;
            let spoken_at = chrono::DateTime::parse_from_rfc3339(&spoken_at_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            result.push(TranscriptEntry {
                id: Some(id),
                session_id,
                channel,
                text,
                spoken_at,
            });
        }

        Ok(result)
    }

    pub fn add_exchange(
        &self,
        session_id: i64,
        query: &str,
        response: &str,
        provider: &str,
        model: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO exchanges (session_id, query, response, provider, model, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                session_id,
                query,
                response,
                provider,
                model,
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn list_sessions(&self) -> Result<Vec<Session>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, title, prompt_template, started_at FROM sessions ORDER BY id DESC",
        )?;

        let sessions = stmt.query_map([], |row| {
            let started_at_str: String = row.get(3)?;
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                started_at_str,
            ))
        })?;

        let mut result = Vec::new();
        for s in sessions {
            let (id, title, prompt_template, started_at_str) = s?;
            let started_at = chrono::DateTime::parse_from_rfc3339(&started_at_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            result.push(Session {
                id,
                title,
                prompt_template,
                started_at,
            });
        }

        Ok(result)
    }

    pub fn search_transcript(&self, query: &str) -> Result<Vec<TranscriptEntry>> {
        let conn = self.conn.lock().unwrap();

        // Simple LIKE search since FTS5 match can be tricky with special chars
        let pattern = format!("%{}%", query);
        let mut stmt = conn.prepare(
            "SELECT id, session_id, channel, text, spoken_at
             FROM transcript
             WHERE text LIKE ?1
             ORDER BY spoken_at DESC
             LIMIT 100",
        )?;

        let entries = stmt.query_map(params![pattern], |row| {
            let spoken_at_str: String = row.get(4)?;
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                spoken_at_str,
            ))
        })?;

        let mut result = Vec::new();
        for entry in entries {
            let (id, session_id, channel, text, spoken_at_str) = entry?;
            let spoken_at = chrono::DateTime::parse_from_rfc3339(&spoken_at_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            result.push(TranscriptEntry {
                id: Some(id),
                session_id,
                channel,
                text,
                spoken_at,
            });
        }

        Ok(result)
    }

    pub fn end_session(&self, session_id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET ended_at = ?1 WHERE id = ?2",
            params![Utc::now().to_rfc3339(), session_id],
        )?;
        Ok(())
    }

    pub fn update_session_title(&self, session_id: i64, title: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET title = ?1 WHERE id = ?2",
            params![title, session_id],
        )?;
        Ok(())
    }

    /// Get the N most recent exchanges across all sessions (chronological order)
    pub fn get_recent_exchanges(&self, limit: usize) -> Result<Vec<(String, String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT query, response, created_at FROM exchanges ORDER BY id DESC LIMIT ?1",
        )?;
        let results = stmt.query_map(params![limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut r: Vec<_> = results.collect::<rusqlite::Result<_>>()?;
        r.reverse(); // chronological
        Ok(r)
    }

    pub fn get_exchanges(&self, session_id: i64) -> Result<Vec<(String, String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT query, response, created_at FROM exchanges WHERE session_id = ?1 ORDER BY id ASC",
        )?;
        let results = stmt.query_map(params![session_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let r: rusqlite::Result<Vec<_>> = results.collect();
        Ok(r?)
    }
}
