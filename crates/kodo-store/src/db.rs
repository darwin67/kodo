use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Pool, Sqlite};
use tracing::debug;

/// The SQLite connection pool.
pub type DbPool = Pool<Sqlite>;

const MIGRATIONS_SQL: &str = include_str!("../migrations/001_initial.sql");

/// Default database path: ~/.local/share/kodo/kodo.db
pub fn default_db_path() -> PathBuf {
    dirs_db().join("kodo.db")
}

fn dirs_db() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".local/share")
        });
    base.join("kodo")
}

/// Open (or create) the database at the given path and run migrations.
pub async fn open(path: &Path) -> Result<DbPool> {
    // Ensure parent directory exists.
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .context("failed to create database directory")?;
    }

    debug!(path = %path.display(), "opening database");

    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await
        .context("failed to open database")?;

    run_migrations(&pool).await?;

    Ok(pool)
}

/// Open an in-memory database (for testing).
pub async fn open_memory() -> Result<DbPool> {
    let options = SqliteConnectOptions::new()
        .filename(":memory:")
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .context("failed to open in-memory database")?;

    run_migrations(&pool).await?;

    Ok(pool)
}

/// Run SQL migrations.
async fn run_migrations(pool: &DbPool) -> Result<()> {
    // Split by semicolons and execute each statement.
    for statement in MIGRATIONS_SQL.split(';') {
        // Strip comments and whitespace.
        let lines: Vec<&str> = statement
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty() && !l.starts_with("--"))
            .collect();
        let stmt = lines.join("\n");
        if stmt.is_empty() {
            continue;
        }
        sqlx::query(&stmt)
            .execute(pool)
            .await
            .with_context(|| format!("migration failed: {}", &stmt[..stmt.len().min(80)]))?;
    }
    debug!("migrations complete");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn open_memory_db() {
        let pool = open_memory().await.unwrap();
        // Verify tables exist by querying sqlite_master.
        let tables: Vec<(String,)> =
            sqlx::query_as("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
                .fetch_all(&pool)
                .await
                .unwrap();

        let table_names: Vec<&str> = tables.iter().map(|t| t.0.as_str()).collect();
        assert!(table_names.contains(&"sessions"));
        assert!(table_names.contains(&"messages"));
        assert!(table_names.contains(&"auth_tokens"));
        assert!(table_names.contains(&"memory"));
    }

    #[tokio::test]
    async fn open_file_db() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = open(&db_path).await.unwrap();
        assert!(db_path.exists());

        // Verify tables.
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='sessions'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count.0, 1);
    }

    #[tokio::test]
    async fn migrations_are_idempotent() {
        let pool = open_memory().await.unwrap();
        // Running migrations again should not fail (CREATE IF NOT EXISTS).
        run_migrations(&pool).await.unwrap();
    }
}
