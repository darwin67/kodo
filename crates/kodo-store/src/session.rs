use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::db::DbPool;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A conversation session — a shared context space for a project.
/// Contains one or more threads, each potentially using a different model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub directory: String,
    pub branch: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// A thread within a session — a single conversation track with a specific model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    pub session_id: String,
    /// The purpose of this thread (e.g. "default", "planner", "executor", "debugger").
    pub role: String,
    pub provider: String,
    pub model: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A stored message within a thread.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    pub id: i64,
    pub thread_id: String,
    pub role: String,
    pub content: String, // JSON-serialized Vec<ContentBlock>
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Session CRUD
// ---------------------------------------------------------------------------

/// Create a new session.
pub async fn create_session(
    pool: &DbPool,
    id: &str,
    directory: &str,
    branch: Option<&str>,
) -> Result<Session> {
    debug!(id, directory, "creating session");

    sqlx::query("INSERT INTO sessions (id, directory, branch) VALUES (?, ?, ?)")
        .bind(id)
        .bind(directory)
        .bind(branch)
        .execute(pool)
        .await?;

    get_session(pool, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("failed to read back created session"))
}

/// Get a session by ID.
pub async fn get_session(pool: &DbPool, id: &str) -> Result<Option<Session>> {
    let row: Option<(String, String, Option<String>, String, String)> = sqlx::query_as(
        "SELECT id, directory, branch, created_at, updated_at FROM sessions WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| Session {
        id: r.0,
        directory: r.1,
        branch: r.2,
        created_at: r.3,
        updated_at: r.4,
    }))
}

/// List sessions, most recently updated first.
pub async fn list_sessions(pool: &DbPool, limit: u32) -> Result<Vec<Session>> {
    let rows: Vec<(String, String, Option<String>, String, String)> = sqlx::query_as(
        "SELECT id, directory, branch, created_at, updated_at \
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
            created_at: r.3,
            updated_at: r.4,
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

/// Delete a session and all its threads/messages (cascaded).
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

/// Fork a session: create a new session with copies of all threads and messages.
pub async fn fork_session(pool: &DbPool, source_id: &str, new_session_id: &str) -> Result<Session> {
    let source = get_session(pool, source_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("source session not found: {source_id}"))?;

    create_session(
        pool,
        new_session_id,
        &source.directory,
        source.branch.as_deref(),
    )
    .await?;

    // Copy all threads, generating new IDs.
    let source_threads = list_threads(pool, source_id).await?;
    for thread in &source_threads {
        let new_thread_id = format!("{new_session_id}:{}", thread.role);
        create_thread(
            pool,
            &new_thread_id,
            new_session_id,
            &thread.role,
            &thread.provider,
            &thread.model,
        )
        .await?;

        // Copy messages from old thread to new thread.
        sqlx::query(
            "INSERT INTO messages (thread_id, role, content, created_at) \
             SELECT ?, role, content, created_at FROM messages WHERE thread_id = ? ORDER BY id",
        )
        .bind(&new_thread_id)
        .bind(&thread.id)
        .execute(pool)
        .await?;
    }

    get_session(pool, new_session_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("failed to read back forked session"))
}

// ---------------------------------------------------------------------------
// Thread CRUD
// ---------------------------------------------------------------------------

/// Create a new thread within a session.
pub async fn create_thread(
    pool: &DbPool,
    id: &str,
    session_id: &str,
    role: &str,
    provider: &str,
    model: &str,
) -> Result<Thread> {
    debug!(id, session_id, role, provider, model, "creating thread");

    sqlx::query(
        "INSERT INTO threads (id, session_id, role, provider, model) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(session_id)
    .bind(role)
    .bind(provider)
    .bind(model)
    .execute(pool)
    .await?;

    // Touch the parent session.
    touch_session(pool, session_id).await?;

    get_thread(pool, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("failed to read back created thread"))
}

/// Get a thread by ID.
pub async fn get_thread(pool: &DbPool, id: &str) -> Result<Option<Thread>> {
    let row: Option<(String, String, String, String, String, String, String)> = sqlx::query_as(
        "SELECT id, session_id, role, provider, model, created_at, updated_at \
         FROM threads WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| Thread {
        id: r.0,
        session_id: r.1,
        role: r.2,
        provider: r.3,
        model: r.4,
        created_at: r.5,
        updated_at: r.6,
    }))
}

/// List all threads in a session.
pub async fn list_threads(pool: &DbPool, session_id: &str) -> Result<Vec<Thread>> {
    let rows: Vec<(String, String, String, String, String, String, String)> = sqlx::query_as(
        "SELECT id, session_id, role, provider, model, created_at, updated_at \
         FROM threads WHERE session_id = ? ORDER BY created_at",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| Thread {
            id: r.0,
            session_id: r.1,
            role: r.2,
            provider: r.3,
            model: r.4,
            created_at: r.5,
            updated_at: r.6,
        })
        .collect())
}

/// Find a thread by session ID and role. Returns the first match.
pub async fn get_thread_by_role(
    pool: &DbPool,
    session_id: &str,
    role: &str,
) -> Result<Option<Thread>> {
    let row: Option<(String, String, String, String, String, String, String)> = sqlx::query_as(
        "SELECT id, session_id, role, provider, model, created_at, updated_at \
         FROM threads WHERE session_id = ? AND role = ? LIMIT 1",
    )
    .bind(session_id)
    .bind(role)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| Thread {
        id: r.0,
        session_id: r.1,
        role: r.2,
        provider: r.3,
        model: r.4,
        created_at: r.5,
        updated_at: r.6,
    }))
}

/// Touch the updated_at timestamp on a thread (and its parent session).
pub async fn touch_thread(pool: &DbPool, thread_id: &str, session_id: &str) -> Result<()> {
    sqlx::query("UPDATE threads SET updated_at = datetime('now') WHERE id = ?")
        .bind(thread_id)
        .execute(pool)
        .await?;
    touch_session(pool, session_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Message CRUD
// ---------------------------------------------------------------------------

/// Save a message to a thread.
pub async fn save_message(
    pool: &DbPool,
    thread_id: &str,
    role: &str,
    content_json: &str,
) -> Result<i64> {
    let result = sqlx::query("INSERT INTO messages (thread_id, role, content) VALUES (?, ?, ?)")
        .bind(thread_id)
        .bind(role)
        .bind(content_json)
        .execute(pool)
        .await?;

    Ok(result.last_insert_rowid())
}

/// Load all messages for a thread, ordered by ID.
pub async fn load_messages(pool: &DbPool, thread_id: &str) -> Result<Vec<StoredMessage>> {
    let rows: Vec<(i64, String, String, String, String)> = sqlx::query_as(
        "SELECT id, thread_id, role, content, created_at \
         FROM messages WHERE thread_id = ? ORDER BY id",
    )
    .bind(thread_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| StoredMessage {
            id: r.0,
            thread_id: r.1,
            role: r.2,
            content: r.3,
            created_at: r.4,
        })
        .collect())
}

/// Load all messages across all threads in a session, ordered chronologically.
pub async fn load_session_messages(pool: &DbPool, session_id: &str) -> Result<Vec<StoredMessage>> {
    let rows: Vec<(i64, String, String, String, String)> = sqlx::query_as(
        "SELECT m.id, m.thread_id, m.role, m.content, m.created_at \
         FROM messages m \
         JOIN threads t ON m.thread_id = t.id \
         WHERE t.session_id = ? \
         ORDER BY m.id",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| StoredMessage {
            id: r.0,
            thread_id: r.1,
            role: r.2,
            content: r.3,
            created_at: r.4,
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_memory;

    // --- Session tests ---

    #[tokio::test]
    async fn create_and_get_session() {
        let pool = open_memory().await.unwrap();
        let session = create_session(&pool, "s1", "/tmp/project", None)
            .await
            .unwrap();
        assert_eq!(session.id, "s1");
        assert_eq!(session.directory, "/tmp/project");

        let fetched = get_session(&pool, "s1").await.unwrap().unwrap();
        assert_eq!(fetched.id, "s1");
    }

    #[tokio::test]
    async fn get_nonexistent_session() {
        let pool = open_memory().await.unwrap();
        assert!(get_session(&pool, "nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_sessions_ordered() {
        let pool = open_memory().await.unwrap();
        create_session(&pool, "s1", "/tmp/a", None).await.unwrap();
        create_session(&pool, "s2", "/tmp/b", None).await.unwrap();
        touch_session(&pool, "s1").await.unwrap();

        let sessions = list_sessions(&pool, 10).await.unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].id, "s1");
    }

    #[tokio::test]
    async fn delete_session_cascades() {
        let pool = open_memory().await.unwrap();
        create_session(&pool, "s1", "/tmp", None).await.unwrap();
        create_thread(&pool, "t1", "s1", "default", "anthropic", "claude")
            .await
            .unwrap();
        save_message(&pool, "t1", "user", r#"[{"type":"text","text":"hi"}]"#)
            .await
            .unwrap();

        delete_session(&pool, "s1").await.unwrap();

        assert!(get_session(&pool, "s1").await.unwrap().is_none());
        assert!(get_thread(&pool, "t1").await.unwrap().is_none());
        assert!(load_messages(&pool, "t1").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn delete_nonexistent_session_fails() {
        let pool = open_memory().await.unwrap();
        assert!(delete_session(&pool, "nope").await.is_err());
    }

    // --- Thread tests ---

    #[tokio::test]
    async fn create_and_get_thread() {
        let pool = open_memory().await.unwrap();
        create_session(&pool, "s1", "/tmp", None).await.unwrap();

        let thread = create_thread(&pool, "t1", "s1", "planner", "anthropic", "claude-opus")
            .await
            .unwrap();
        assert_eq!(thread.id, "t1");
        assert_eq!(thread.role, "planner");
        assert_eq!(thread.provider, "anthropic");
        assert_eq!(thread.model, "claude-opus");
    }

    #[tokio::test]
    async fn multiple_threads_in_session() {
        let pool = open_memory().await.unwrap();
        create_session(&pool, "s1", "/tmp", None).await.unwrap();

        create_thread(&pool, "t1", "s1", "planner", "anthropic", "claude-opus")
            .await
            .unwrap();
        create_thread(&pool, "t2", "s1", "executor", "anthropic", "claude-sonnet")
            .await
            .unwrap();
        create_thread(&pool, "t3", "s1", "debugger", "openai", "gpt-4o")
            .await
            .unwrap();

        let threads = list_threads(&pool, "s1").await.unwrap();
        assert_eq!(threads.len(), 3);

        let roles: Vec<&str> = threads.iter().map(|t| t.role.as_str()).collect();
        assert!(roles.contains(&"planner"));
        assert!(roles.contains(&"executor"));
        assert!(roles.contains(&"debugger"));

        // Each thread has a different model.
        let models: Vec<&str> = threads.iter().map(|t| t.model.as_str()).collect();
        assert!(models.contains(&"claude-opus"));
        assert!(models.contains(&"claude-sonnet"));
        assert!(models.contains(&"gpt-4o"));
    }

    #[tokio::test]
    async fn get_thread_by_role_works() {
        let pool = open_memory().await.unwrap();
        create_session(&pool, "s1", "/tmp", None).await.unwrap();
        create_thread(&pool, "t1", "s1", "planner", "anthropic", "opus")
            .await
            .unwrap();
        create_thread(&pool, "t2", "s1", "executor", "anthropic", "sonnet")
            .await
            .unwrap();

        let planner = get_thread_by_role(&pool, "s1", "planner")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(planner.id, "t1");
        assert_eq!(planner.model, "opus");

        let executor = get_thread_by_role(&pool, "s1", "executor")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(executor.id, "t2");

        assert!(
            get_thread_by_role(&pool, "s1", "nonexistent")
                .await
                .unwrap()
                .is_none()
        );
    }

    // --- Message tests ---

    #[tokio::test]
    async fn save_and_load_thread_messages() {
        let pool = open_memory().await.unwrap();
        create_session(&pool, "s1", "/tmp", None).await.unwrap();
        create_thread(&pool, "t1", "s1", "default", "anthropic", "claude")
            .await
            .unwrap();

        save_message(&pool, "t1", "user", r#"[{"type":"text","text":"hello"}]"#)
            .await
            .unwrap();
        save_message(
            &pool,
            "t1",
            "assistant",
            r#"[{"type":"text","text":"hi there"}]"#,
        )
        .await
        .unwrap();

        let messages = load_messages(&pool, "t1").await.unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[1].role, "assistant");
    }

    #[tokio::test]
    async fn messages_isolated_per_thread() {
        let pool = open_memory().await.unwrap();
        create_session(&pool, "s1", "/tmp", None).await.unwrap();
        create_thread(&pool, "t1", "s1", "planner", "anthropic", "opus")
            .await
            .unwrap();
        create_thread(&pool, "t2", "s1", "executor", "anthropic", "sonnet")
            .await
            .unwrap();

        save_message(&pool, "t1", "user", r#"[{"type":"text","text":"plan"}]"#)
            .await
            .unwrap();
        save_message(&pool, "t2", "user", r#"[{"type":"text","text":"execute"}]"#)
            .await
            .unwrap();

        let t1_msgs = load_messages(&pool, "t1").await.unwrap();
        let t2_msgs = load_messages(&pool, "t2").await.unwrap();

        assert_eq!(t1_msgs.len(), 1);
        assert_eq!(t2_msgs.len(), 1);
        assert!(t1_msgs[0].content.contains("plan"));
        assert!(t2_msgs[0].content.contains("execute"));
    }

    #[tokio::test]
    async fn load_all_session_messages() {
        let pool = open_memory().await.unwrap();
        create_session(&pool, "s1", "/tmp", None).await.unwrap();
        create_thread(&pool, "t1", "s1", "planner", "anthropic", "opus")
            .await
            .unwrap();
        create_thread(&pool, "t2", "s1", "executor", "anthropic", "sonnet")
            .await
            .unwrap();

        save_message(&pool, "t1", "user", r#"[{"type":"text","text":"plan"}]"#)
            .await
            .unwrap();
        save_message(&pool, "t2", "user", r#"[{"type":"text","text":"exec"}]"#)
            .await
            .unwrap();
        save_message(&pool, "t1", "assistant", r#"[{"type":"text","text":"ok"}]"#)
            .await
            .unwrap();

        let all = load_session_messages(&pool, "s1").await.unwrap();
        assert_eq!(all.len(), 3);
        // Should be chronologically ordered by id.
        assert!(all[0].content.contains("plan"));
        assert!(all[1].content.contains("exec"));
        assert!(all[2].content.contains("ok"));
    }

    // --- Fork tests ---

    #[tokio::test]
    async fn fork_session_copies_threads_and_messages() {
        let pool = open_memory().await.unwrap();
        create_session(&pool, "s1", "/tmp", Some("main"))
            .await
            .unwrap();
        create_thread(&pool, "t1", "s1", "planner", "anthropic", "opus")
            .await
            .unwrap();
        create_thread(&pool, "t2", "s1", "executor", "openai", "gpt-4o")
            .await
            .unwrap();

        save_message(&pool, "t1", "user", r#"[{"type":"text","text":"msg1"}]"#)
            .await
            .unwrap();
        save_message(&pool, "t2", "user", r#"[{"type":"text","text":"msg2"}]"#)
            .await
            .unwrap();

        let forked = fork_session(&pool, "s1", "s2").await.unwrap();
        assert_eq!(forked.id, "s2");

        let forked_threads = list_threads(&pool, "s2").await.unwrap();
        assert_eq!(forked_threads.len(), 2);

        let forked_roles: Vec<&str> = forked_threads.iter().map(|t| t.role.as_str()).collect();
        assert!(forked_roles.contains(&"planner"));
        assert!(forked_roles.contains(&"executor"));

        // Verify messages were copied.
        let all_msgs = load_session_messages(&pool, "s2").await.unwrap();
        assert_eq!(all_msgs.len(), 2);
    }
}
