use anyhow::{Context, Result};
use rusqlite::Connection;
use tracing::info;

/// Simple schema migration system.
/// Each migration is a (version, description, SQL) tuple.
/// Migrations run exactly once, tracked in the `schema_version` table.
const MIGRATIONS: &[(i64, &str, &str)] = &[
    (
        1,
        "initial schema",
        "", // v1 = existing tables created by init_schema() calls
    ),
    (
        2,
        "brain tables",
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
    ),
];

/// Ensure the schema_version table exists and run any pending migrations.
pub fn run_migrations(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER PRIMARY KEY,
            description TEXT NOT NULL,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )
    .context("Failed to create schema_version table")?;

    let current: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    for &(version, description, sql) in MIGRATIONS {
        if version > current {
            if !sql.is_empty() {
                conn.execute_batch(sql)
                    .with_context(|| format!("Migration {} failed: {}", version, description))?;
            }
            conn.execute(
                "INSERT INTO schema_version (version, description) VALUES (?1, ?2)",
                rusqlite::params![version, description],
            )?;
            info!(version, description, "Migration applied");
        }
    }

    Ok(())
}

/// Get the current schema version.
pub fn current_version(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |row| row.get(0),
    )
    .unwrap_or(0)
}
