use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::db::DbPool;

/// Raw session data tuple from database queries.
type SessionRow = (
    String,
    String,
    Option<String>,
    String,
    String,
    String,
    String,
);

/// A conversation session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub directory: String,
    pub branch: Option<String>,
    pub provider: String,
    pub model: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A stored message within a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    pub id: i64,
    pub session_id: String,
    pub role: String,
    pub content: String, // JSON-serialized Vec<ContentBlock>
    pub created_at: String,
}

/// Create a new session and return it.
pub async fn create_session(
    pool: &DbPool,
    id: &str,
    directory: &str,
    branch: Option<&str>,
    provider: &str,
    model: &str,
) -> Result<Session> {
    debug!(id, directory, provider, model, "creating session");

    sqlx::query(
        "INSERT INTO sessions (id, directory, branch, provider, model) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(directory)
    .bind(branch)
    .bind(provider)
    .bind(model)
    .execute(pool)
    .await?;

    get_session(pool, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("failed to read back created session"))
}

/// Get a session by ID.
pub async fn get_session(pool: &DbPool, id: &str) -> Result<Option<Session>> {
    let row: Option<SessionRow> =
        sqlx::query_as(
            "SELECT id, directory, branch, provider, model, created_at, updated_at FROM sessions WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(pool)
        .await?;

    Ok(row.map(|r| Session {
        id: r.0,
        directory: r.1,
        branch: r.2,
        provider: r.3,
        model: r.4,
        created_at: r.5,
        updated_at: r.6,
    }))
}

/// List sessions, most recently updated first.
pub async fn list_sessions(pool: &DbPool, limit: u32) -> Result<Vec<Session>> {
    let rows: Vec<SessionRow> = sqlx::query_as(
        "SELECT id, directory, branch, provider, model, created_at, updated_at \
             FROM sessions ORDER BY updated_at DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| Session {
            id: r.0,
            directory: r.1,
            branch: r.2,
            provider: r.3,
            model: r.4,
            created_at: r.5,
            updated_at: r.6,
        })
        .collect())
}

/// Touch the updated_at timestamp on a session.
pub async fn touch_session(pool: &DbPool, id: &str) -> Result<()> {
    sqlx::query("UPDATE sessions SET updated_at = datetime('now') WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Save a message to a session.
pub async fn save_message(
    pool: &DbPool,
    session_id: &str,
    role: &str,
    content_json: &str,
) -> Result<i64> {
    let result = sqlx::query("INSERT INTO messages (session_id, role, content) VALUES (?, ?, ?)")
        .bind(session_id)
        .bind(role)
        .bind(content_json)
        .execute(pool)
        .await?;

    touch_session(pool, session_id).await?;

    Ok(result.last_insert_rowid())
}

/// Load all messages for a session, ordered by ID.
pub async fn load_messages(pool: &DbPool, session_id: &str) -> Result<Vec<StoredMessage>> {
    let rows: Vec<(i64, String, String, String, String)> = sqlx::query_as(
        "SELECT id, session_id, role, content, created_at \
         FROM messages WHERE session_id = ? ORDER BY id",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| StoredMessage {
            id: r.0,
            session_id: r.1,
            role: r.2,
            content: r.3,
            created_at: r.4,
        })
        .collect())
}

/// Fork a session: create a new session with copies of all messages.
pub async fn fork_session(pool: &DbPool, source_id: &str, new_id: &str) -> Result<Session> {
    let source = get_session(pool, source_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("source session not found: {source_id}"))?;

    // Create the new session.
    create_session(
        pool,
        new_id,
        &source.directory,
        source.branch.as_deref(),
        &source.provider,
        &source.model,
    )
    .await?;

    // Copy all messages.
    sqlx::query(
        "INSERT INTO messages (session_id, role, content, created_at) \
         SELECT ?, role, content, created_at FROM messages WHERE session_id = ? ORDER BY id",
    )
    .bind(new_id)
    .bind(source_id)
    .execute(pool)
    .await?;

    get_session(pool, new_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("failed to read back forked session"))
}

/// Delete a session and its messages.
pub async fn delete_session(pool: &DbPool, id: &str) -> Result<()> {
    let result = sqlx::query("DELETE FROM sessions WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;

    if result.rows_affected() == 0 {
        bail!("session not found: {id}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_memory;

    #[tokio::test]
    async fn create_and_get_session() {
        let pool = open_memory().await.unwrap();
        let session = create_session(&pool, "s1", "/tmp/project", None, "anthropic", "claude")
            .await
            .unwrap();
        assert_eq!(session.id, "s1");
        assert_eq!(session.provider, "anthropic");

        let fetched = get_session(&pool, "s1").await.unwrap().unwrap();
        assert_eq!(fetched.id, "s1");
    }

    #[tokio::test]
    async fn get_nonexistent_session() {
        let pool = open_memory().await.unwrap();
        let result = get_session(&pool, "nope").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn list_sessions_ordered() {
        let pool = open_memory().await.unwrap();
        create_session(&pool, "s1", "/tmp/a", None, "openai", "gpt-4o")
            .await
            .unwrap();
        create_session(&pool, "s2", "/tmp/b", None, "anthropic", "claude")
            .await
            .unwrap();

        // Touch s1 to make it more recent.
        touch_session(&pool, "s1").await.unwrap();

        let sessions = list_sessions(&pool, 10).await.unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].id, "s1"); // Most recently updated.
    }

    #[tokio::test]
    async fn save_and_load_messages() {
        let pool = open_memory().await.unwrap();
        create_session(&pool, "s1", "/tmp", None, "anthropic", "claude")
            .await
            .unwrap();

        save_message(&pool, "s1", "user", r#"[{"type":"text","text":"hello"}]"#)
            .await
            .unwrap();
        save_message(
            &pool,
            "s1",
            "assistant",
            r#"[{"type":"text","text":"hi there"}]"#,
        )
        .await
        .unwrap();

        let messages = load_messages(&pool, "s1").await.unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[1].role, "assistant");
        assert!(messages[0].content.contains("hello"));
    }

    #[tokio::test]
    async fn fork_session_copies_messages() {
        let pool = open_memory().await.unwrap();
        create_session(&pool, "s1", "/tmp", Some("main"), "anthropic", "claude")
            .await
            .unwrap();
        save_message(&pool, "s1", "user", r#"[{"type":"text","text":"msg1"}]"#)
            .await
            .unwrap();
        save_message(
            &pool,
            "s1",
            "assistant",
            r#"[{"type":"text","text":"msg2"}]"#,
        )
        .await
        .unwrap();

        let forked = fork_session(&pool, "s1", "s2").await.unwrap();
        assert_eq!(forked.id, "s2");
        assert_eq!(forked.provider, "anthropic");
        assert_eq!(forked.branch, Some("main".into()));

        let messages = load_messages(&pool, "s2").await.unwrap();
        assert_eq!(messages.len(), 2);
    }

    #[tokio::test]
    async fn delete_session_cascades() {
        let pool = open_memory().await.unwrap();
        create_session(&pool, "s1", "/tmp", None, "openai", "gpt-4o")
            .await
            .unwrap();
        save_message(&pool, "s1", "user", r#"[{"type":"text","text":"hi"}]"#)
            .await
            .unwrap();

        delete_session(&pool, "s1").await.unwrap();

        assert!(get_session(&pool, "s1").await.unwrap().is_none());
        let messages = load_messages(&pool, "s1").await.unwrap();
        assert!(messages.is_empty());
    }

    #[tokio::test]
    async fn delete_nonexistent_session_fails() {
        let pool = open_memory().await.unwrap();
        let result = delete_session(&pool, "nope").await;
        assert!(result.is_err());
    }
}
