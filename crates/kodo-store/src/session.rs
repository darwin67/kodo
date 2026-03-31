use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use tracing::debug;
use uuid::Uuid;

use crate::db::DbPool;

// ---------------------------------------------------------------------------
// ID generation
// ---------------------------------------------------------------------------

/// Generate a new UUIDv7 string. Time-sortable, embeds creation timestamp.
pub fn new_id() -> String {
    Uuid::now_v7().to_string()
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A conversation session — a shared context space for a project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub name: String,
    pub directory: String,
    pub branch: Option<String>,
    pub updated_at: String,
}

/// A thread within a session — a conversation track with a specific model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub provider: String,
    pub model: String,
    pub updated_at: String,
}

/// A stored message within a thread.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    pub id: String,
    pub thread_id: String,
    pub role: String,
    pub content: String,
}

// ---------------------------------------------------------------------------
// Session CRUD
// ---------------------------------------------------------------------------

/// Create a new session. Returns the created session with a generated UUIDv7.
pub async fn create_session(
    pool: &DbPool,
    name: &str,
    directory: &str,
    branch: Option<&str>,
) -> Result<Session> {
    let id = new_id();
    debug!(id = %id, name, directory, "creating session");

    sqlx::query("INSERT INTO sessions (id, name, directory, branch) VALUES (?, ?, ?, ?)")
        .bind(&id)
        .bind(name)
        .bind(directory)
        .bind(branch)
        .execute(pool)
        .await?;

    get_session(pool, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("failed to read back created session"))
}

/// Get a session by ID.
pub async fn get_session(pool: &DbPool, id: &str) -> Result<Option<Session>> {
    let row: Option<(String, String, String, Option<String>, String)> =
        sqlx::query_as("SELECT id, name, directory, branch, updated_at FROM sessions WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?;

    Ok(row.map(into_session))
}

/// Find a session by name (exact match).
pub async fn get_session_by_name(pool: &DbPool, name: &str) -> Result<Option<Session>> {
    let row: Option<(String, String, String, Option<String>, String)> = sqlx::query_as(
        "SELECT id, name, directory, branch, updated_at FROM sessions WHERE name = ? LIMIT 1",
    )
    .bind(name)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(into_session))
}

/// Search sessions by name substring (case-insensitive).
pub async fn search_sessions(pool: &DbPool, query: &str, limit: u32) -> Result<Vec<Session>> {
    let pattern = format!("%{query}%");
    let rows: Vec<(String, String, String, Option<String>, String)> = sqlx::query_as(
        "SELECT id, name, directory, branch, updated_at \
         FROM sessions WHERE name LIKE ? ORDER BY updated_at DESC LIMIT ?",
    )
    .bind(&pattern)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(into_session).collect())
}

/// List sessions, most recently updated first.
pub async fn list_sessions(pool: &DbPool, limit: u32) -> Result<Vec<Session>> {
    let rows: Vec<(String, String, String, Option<String>, String)> = sqlx::query_as(
        "SELECT id, name, directory, branch, updated_at \
         FROM sessions ORDER BY updated_at DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(into_session).collect())
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

/// Fork a session: new session with copies of all threads and messages.
pub async fn fork_session(pool: &DbPool, source_id: &str, new_name: &str) -> Result<Session> {
    let source = get_session(pool, source_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("source session not found: {source_id}"))?;

    let new_session =
        create_session(pool, new_name, &source.directory, source.branch.as_deref()).await?;

    let source_threads = list_threads(pool, source_id).await?;
    for thread in &source_threads {
        let new_thread = create_thread(
            pool,
            &new_session.id,
            &thread.role,
            &thread.provider,
            &thread.model,
        )
        .await?;

        // Copy messages, generating new UUIDv7 IDs.
        let msgs = load_messages(pool, &thread.id).await?;
        for msg in &msgs {
            save_message(pool, &new_thread.id, &msg.role, &msg.content).await?;
        }
    }

    get_session(pool, &new_session.id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("failed to read back forked session"))
}

fn into_session(r: (String, String, String, Option<String>, String)) -> Session {
    Session {
        id: r.0,
        name: r.1,
        directory: r.2,
        branch: r.3,
        updated_at: r.4,
    }
}

// ---------------------------------------------------------------------------
// Thread CRUD
// ---------------------------------------------------------------------------

/// Create a new thread within a session. ID is auto-generated UUIDv7.
pub async fn create_thread(
    pool: &DbPool,
    session_id: &str,
    role: &str,
    provider: &str,
    model: &str,
) -> Result<Thread> {
    let id = new_id();
    debug!(id = %id, session_id, role, provider, model, "creating thread");

    sqlx::query(
        "INSERT INTO threads (id, session_id, role, provider, model) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(session_id)
    .bind(role)
    .bind(provider)
    .bind(model)
    .execute(pool)
    .await?;

    touch_session(pool, session_id).await?;

    get_thread(pool, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("failed to read back created thread"))
}

/// Get a thread by ID.
pub async fn get_thread(pool: &DbPool, id: &str) -> Result<Option<Thread>> {
    let row: Option<(String, String, String, String, String, String)> = sqlx::query_as(
        "SELECT id, session_id, role, provider, model, updated_at FROM threads WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(into_thread))
}

/// List all threads in a session, ordered by ID (time-sorted via UUIDv7).
pub async fn list_threads(pool: &DbPool, session_id: &str) -> Result<Vec<Thread>> {
    let rows: Vec<(String, String, String, String, String, String)> = sqlx::query_as(
        "SELECT id, session_id, role, provider, model, updated_at \
         FROM threads WHERE session_id = ? ORDER BY id",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(into_thread).collect())
}

/// Find a thread by session ID and role.
pub async fn get_thread_by_role(
    pool: &DbPool,
    session_id: &str,
    role: &str,
) -> Result<Option<Thread>> {
    let row: Option<(String, String, String, String, String, String)> = sqlx::query_as(
        "SELECT id, session_id, role, provider, model, updated_at \
         FROM threads WHERE session_id = ? AND role = ? LIMIT 1",
    )
    .bind(session_id)
    .bind(role)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(into_thread))
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

fn into_thread(r: (String, String, String, String, String, String)) -> Thread {
    Thread {
        id: r.0,
        session_id: r.1,
        role: r.2,
        provider: r.3,
        model: r.4,
        updated_at: r.5,
    }
}

// ---------------------------------------------------------------------------
// Message CRUD
// ---------------------------------------------------------------------------

/// Save a message to a thread. ID is auto-generated UUIDv7.
pub async fn save_message(
    pool: &DbPool,
    thread_id: &str,
    role: &str,
    content_json: &str,
) -> Result<String> {
    let id = new_id();

    sqlx::query("INSERT INTO messages (id, thread_id, role, content) VALUES (?, ?, ?, ?)")
        .bind(&id)
        .bind(thread_id)
        .bind(role)
        .bind(content_json)
        .execute(pool)
        .await?;

    Ok(id)
}

/// Load all messages for a thread, ordered by ID (time-sorted via UUIDv7).
pub async fn load_messages(pool: &DbPool, thread_id: &str) -> Result<Vec<StoredMessage>> {
    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT id, thread_id, role, content FROM messages WHERE thread_id = ? ORDER BY id",
    )
    .bind(thread_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(into_message).collect())
}

/// Load all messages across all threads in a session, ordered by ID.
pub async fn load_session_messages(pool: &DbPool, session_id: &str) -> Result<Vec<StoredMessage>> {
    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT m.id, m.thread_id, m.role, m.content \
         FROM messages m \
         JOIN threads t ON m.thread_id = t.id \
         WHERE t.session_id = ? \
         ORDER BY m.id",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(into_message).collect())
}

fn into_message(r: (String, String, String, String)) -> StoredMessage {
    StoredMessage {
        id: r.0,
        thread_id: r.1,
        role: r.2,
        content: r.3,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_memory;

    // --- ID generation ---

    #[test]
    fn new_id_is_valid_uuid() {
        let id = new_id();
        assert!(Uuid::parse_str(&id).is_ok());
    }

    #[test]
    fn new_id_is_v7() {
        let id = new_id();
        let uuid = Uuid::parse_str(&id).unwrap();
        assert_eq!(uuid.get_version(), Some(uuid::Version::SortRand));
    }

    #[test]
    fn new_ids_are_time_sorted() {
        let id1 = new_id();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let id2 = new_id();
        assert!(
            id1 < id2,
            "UUIDv7 should be lexicographically sortable by time"
        );
    }

    // --- Session tests ---

    #[tokio::test]
    async fn create_and_get_session() {
        let pool = open_memory().await.unwrap();
        let session = create_session(&pool, "my-project", "/tmp/project", None)
            .await
            .unwrap();
        assert_eq!(session.name, "my-project");
        assert_eq!(session.directory, "/tmp/project");
        assert!(Uuid::parse_str(&session.id).is_ok());

        let fetched = get_session(&pool, &session.id).await.unwrap().unwrap();
        assert_eq!(fetched.name, "my-project");
    }

    #[tokio::test]
    async fn get_session_by_name_works() {
        let pool = open_memory().await.unwrap();
        let session = create_session(&pool, "kodo-refactor", "/tmp", None)
            .await
            .unwrap();

        let found = get_session_by_name(&pool, "kodo-refactor")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.id, session.id);

        assert!(
            get_session_by_name(&pool, "nonexistent")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn search_sessions_by_name() {
        let pool = open_memory().await.unwrap();
        create_session(&pool, "kodo-phase-1", "/tmp/a", None)
            .await
            .unwrap();
        create_session(&pool, "kodo-phase-2", "/tmp/b", None)
            .await
            .unwrap();
        create_session(&pool, "other-project", "/tmp/c", None)
            .await
            .unwrap();

        let results = search_sessions(&pool, "kodo", 10).await.unwrap();
        assert_eq!(results.len(), 2);

        let results = search_sessions(&pool, "phase-1", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "kodo-phase-1");

        let results = search_sessions(&pool, "nope", 10).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn get_nonexistent_session() {
        let pool = open_memory().await.unwrap();
        assert!(get_session(&pool, "nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_sessions_ordered() {
        let pool = open_memory().await.unwrap();
        let s1 = create_session(&pool, "first", "/tmp/a", None)
            .await
            .unwrap();
        create_session(&pool, "second", "/tmp/b", None)
            .await
            .unwrap();
        touch_session(&pool, &s1.id).await.unwrap();

        let sessions = list_sessions(&pool, 10).await.unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].name, "first");
    }

    #[tokio::test]
    async fn delete_session_cascades() {
        let pool = open_memory().await.unwrap();
        let session = create_session(&pool, "doomed", "/tmp", None).await.unwrap();
        let thread = create_thread(&pool, &session.id, "default", "anthropic", "claude")
            .await
            .unwrap();
        save_message(
            &pool,
            &thread.id,
            "user",
            r#"[{"type":"text","text":"hi"}]"#,
        )
        .await
        .unwrap();

        delete_session(&pool, &session.id).await.unwrap();

        assert!(get_session(&pool, &session.id).await.unwrap().is_none());
        assert!(get_thread(&pool, &thread.id).await.unwrap().is_none());
        assert!(load_messages(&pool, &thread.id).await.unwrap().is_empty());
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
        let session = create_session(&pool, "test", "/tmp", None).await.unwrap();
        let thread = create_thread(&pool, &session.id, "planner", "anthropic", "claude-opus")
            .await
            .unwrap();

        assert!(Uuid::parse_str(&thread.id).is_ok());
        assert_eq!(thread.role, "planner");
        assert_eq!(thread.provider, "anthropic");
        assert_eq!(thread.model, "claude-opus");
    }

    #[tokio::test]
    async fn multiple_threads_in_session() {
        let pool = open_memory().await.unwrap();
        let session = create_session(&pool, "multi", "/tmp", None).await.unwrap();

        create_thread(&pool, &session.id, "planner", "anthropic", "opus")
            .await
            .unwrap();
        create_thread(&pool, &session.id, "executor", "anthropic", "sonnet")
            .await
            .unwrap();
        create_thread(&pool, &session.id, "debugger", "openai", "gpt-4o")
            .await
            .unwrap();

        let threads = list_threads(&pool, &session.id).await.unwrap();
        assert_eq!(threads.len(), 3);

        let roles: Vec<&str> = threads.iter().map(|t| t.role.as_str()).collect();
        assert!(roles.contains(&"planner"));
        assert!(roles.contains(&"executor"));
        assert!(roles.contains(&"debugger"));
    }

    #[tokio::test]
    async fn get_thread_by_role_works() {
        let pool = open_memory().await.unwrap();
        let session = create_session(&pool, "test", "/tmp", None).await.unwrap();
        create_thread(&pool, &session.id, "planner", "anthropic", "opus")
            .await
            .unwrap();
        create_thread(&pool, &session.id, "executor", "anthropic", "sonnet")
            .await
            .unwrap();

        let planner = get_thread_by_role(&pool, &session.id, "planner")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(planner.model, "opus");

        assert!(
            get_thread_by_role(&pool, &session.id, "nonexistent")
                .await
                .unwrap()
                .is_none()
        );
    }

    // --- Message tests ---

    #[tokio::test]
    async fn save_and_load_messages() {
        let pool = open_memory().await.unwrap();
        let session = create_session(&pool, "test", "/tmp", None).await.unwrap();
        let thread = create_thread(&pool, &session.id, "default", "anthropic", "claude")
            .await
            .unwrap();

        let msg_id = save_message(
            &pool,
            &thread.id,
            "user",
            r#"[{"type":"text","text":"hello"}]"#,
        )
        .await
        .unwrap();
        assert!(Uuid::parse_str(&msg_id).is_ok());

        save_message(
            &pool,
            &thread.id,
            "assistant",
            r#"[{"type":"text","text":"hi"}]"#,
        )
        .await
        .unwrap();

        let messages = load_messages(&pool, &thread.id).await.unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[1].role, "assistant");
    }

    #[tokio::test]
    async fn messages_isolated_per_thread() {
        let pool = open_memory().await.unwrap();
        let session = create_session(&pool, "test", "/tmp", None).await.unwrap();
        let t1 = create_thread(&pool, &session.id, "planner", "anthropic", "opus")
            .await
            .unwrap();
        let t2 = create_thread(&pool, &session.id, "executor", "anthropic", "sonnet")
            .await
            .unwrap();

        save_message(&pool, &t1.id, "user", r#"[{"type":"text","text":"plan"}]"#)
            .await
            .unwrap();
        save_message(&pool, &t2.id, "user", r#"[{"type":"text","text":"exec"}]"#)
            .await
            .unwrap();

        assert_eq!(load_messages(&pool, &t1.id).await.unwrap().len(), 1);
        assert_eq!(load_messages(&pool, &t2.id).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn load_all_session_messages() {
        let pool = open_memory().await.unwrap();
        let session = create_session(&pool, "test", "/tmp", None).await.unwrap();
        let t1 = create_thread(&pool, &session.id, "planner", "anthropic", "opus")
            .await
            .unwrap();
        let t2 = create_thread(&pool, &session.id, "executor", "anthropic", "sonnet")
            .await
            .unwrap();

        save_message(&pool, &t1.id, "user", r#"[{"type":"text","text":"plan"}]"#)
            .await
            .unwrap();
        save_message(&pool, &t2.id, "user", r#"[{"type":"text","text":"exec"}]"#)
            .await
            .unwrap();
        save_message(
            &pool,
            &t1.id,
            "assistant",
            r#"[{"type":"text","text":"ok"}]"#,
        )
        .await
        .unwrap();

        let all = load_session_messages(&pool, &session.id).await.unwrap();
        assert_eq!(all.len(), 3);
    }

    // --- Fork tests ---

    #[tokio::test]
    async fn fork_session_copies_threads_and_messages() {
        let pool = open_memory().await.unwrap();
        let session = create_session(&pool, "original", "/tmp", Some("main"))
            .await
            .unwrap();
        let t1 = create_thread(&pool, &session.id, "planner", "anthropic", "opus")
            .await
            .unwrap();
        let t2 = create_thread(&pool, &session.id, "executor", "openai", "gpt-4o")
            .await
            .unwrap();

        save_message(&pool, &t1.id, "user", r#"[{"type":"text","text":"msg1"}]"#)
            .await
            .unwrap();
        save_message(&pool, &t2.id, "user", r#"[{"type":"text","text":"msg2"}]"#)
            .await
            .unwrap();

        let forked = fork_session(&pool, &session.id, "forked-copy")
            .await
            .unwrap();
        assert_eq!(forked.name, "forked-copy");
        assert_ne!(forked.id, session.id);

        let forked_threads = list_threads(&pool, &forked.id).await.unwrap();
        assert_eq!(forked_threads.len(), 2);

        let all_msgs = load_session_messages(&pool, &forked.id).await.unwrap();
        assert_eq!(all_msgs.len(), 2);
    }
}
