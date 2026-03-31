use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use tracing::debug;
use uuid::Uuid;

use crate::db::DbPool;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// A thread is considered "live" if its heartbeat is within this many seconds.
pub const HEARTBEAT_STALE_SECS: i64 = 60;

/// Recommended heartbeat interval (half the staleness threshold).
pub const HEARTBEAT_INTERVAL_SECS: u64 = 30;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Active,
    Archived,
}

impl std::fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active => write!(f, "active"),
            Self::Archived => write!(f, "archived"),
        }
    }
}

/// A conversation session — a shared context space for a project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub name: String,
    pub directory: String,
    pub branch: Option<String>,
    pub status: SessionStatus,
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
    pub heartbeat_at: Option<String>,
    pub updated_at: String,
}

impl Thread {
    /// Whether this thread has a live heartbeat (not stale).
    /// Returns false if no heartbeat has been set.
    pub fn is_live(&self) -> bool {
        match &self.heartbeat_at {
            None => false,
            Some(hb) => {
                // Parse the heartbeat timestamp and check against staleness.
                chrono::NaiveDateTime::parse_from_str(hb, "%Y-%m-%d %H:%M:%S")
                    .map(|dt| {
                        let now = chrono::Utc::now().naive_utc();
                        (now - dt).num_seconds() < HEARTBEAT_STALE_SECS
                    })
                    .unwrap_or(false)
            }
        }
    }
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

/// Create a new session.
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
    let row: Option<(String, String, String, Option<String>, String, String)> = sqlx::query_as(
        "SELECT id, name, directory, branch, status, updated_at FROM sessions WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(into_session))
}

/// Find a session by name (exact match).
pub async fn get_session_by_name(pool: &DbPool, name: &str) -> Result<Option<Session>> {
    let row: Option<(String, String, String, Option<String>, String, String)> = sqlx::query_as(
        "SELECT id, name, directory, branch, status, updated_at \
         FROM sessions WHERE name = ? LIMIT 1",
    )
    .bind(name)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(into_session))
}

/// Search sessions by name substring (case-insensitive).
pub async fn search_sessions(pool: &DbPool, query: &str, limit: u32) -> Result<Vec<Session>> {
    let pattern = format!("%{query}%");
    let rows: Vec<(String, String, String, Option<String>, String, String)> = sqlx::query_as(
        "SELECT id, name, directory, branch, status, updated_at \
         FROM sessions WHERE name LIKE ? ORDER BY updated_at DESC LIMIT ?",
    )
    .bind(&pattern)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(into_session).collect())
}

/// List sessions, most recently updated first. Optionally filter by status.
pub async fn list_sessions(
    pool: &DbPool,
    status: Option<SessionStatus>,
    limit: u32,
) -> Result<Vec<Session>> {
    let rows: Vec<(String, String, String, Option<String>, String, String)> = match status {
        Some(s) => {
            sqlx::query_as(
                "SELECT id, name, directory, branch, status, updated_at \
                 FROM sessions WHERE status = ? ORDER BY updated_at DESC LIMIT ?",
            )
            .bind(s.to_string())
            .bind(limit)
            .fetch_all(pool)
            .await?
        }
        None => {
            sqlx::query_as(
                "SELECT id, name, directory, branch, status, updated_at \
                 FROM sessions ORDER BY updated_at DESC LIMIT ?",
            )
            .bind(limit)
            .fetch_all(pool)
            .await?
        }
    };

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

/// Archive a session.
pub async fn archive_session(pool: &DbPool, id: &str) -> Result<()> {
    sqlx::query(
        "UPDATE sessions SET status = 'archived', updated_at = datetime('now') WHERE id = ?",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Reactivate an archived session.
pub async fn activate_session(pool: &DbPool, id: &str) -> Result<()> {
    sqlx::query("UPDATE sessions SET status = 'active', updated_at = datetime('now') WHERE id = ?")
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

/// Fork a session.
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

        let msgs = load_messages(pool, &thread.id).await?;
        for msg in &msgs {
            save_message(pool, &new_thread.id, &msg.role, &msg.content).await?;
        }
    }

    get_session(pool, &new_session.id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("failed to read back forked session"))
}

/// Sweep stale sessions: archive any active session where all threads
/// have stale heartbeats (or no heartbeat at all).
pub async fn sweep_stale_sessions(pool: &DbPool) -> Result<u32> {
    // Find active sessions where no thread has a recent heartbeat.
    let stale_ids: Vec<(String,)> = sqlx::query_as(
        "SELECT s.id FROM sessions s \
         WHERE s.status = 'active' \
         AND NOT EXISTS ( \
           SELECT 1 FROM threads t \
           WHERE t.session_id = s.id \
           AND t.heartbeat_at IS NOT NULL \
           AND t.heartbeat_at > datetime('now', ?) \
         )",
    )
    .bind(format!("-{HEARTBEAT_STALE_SECS} seconds"))
    .fetch_all(pool)
    .await?;

    let count = stale_ids.len() as u32;
    for (id,) in &stale_ids {
        debug!(session_id = %id, "archiving stale session");
        archive_session(pool, id).await?;
    }

    Ok(count)
}

fn into_session(r: (String, String, String, Option<String>, String, String)) -> Session {
    Session {
        id: r.0,
        name: r.1,
        directory: r.2,
        branch: r.3,
        status: match r.4.as_str() {
            "archived" => SessionStatus::Archived,
            _ => SessionStatus::Active,
        },
        updated_at: r.5,
    }
}

// ---------------------------------------------------------------------------
// Thread CRUD
// ---------------------------------------------------------------------------

type ThreadRow = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    String,
);

/// Create a new thread within a session.
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
    let row: Option<ThreadRow> = sqlx::query_as(
        "SELECT id, session_id, role, provider, model, heartbeat_at, updated_at \
             FROM threads WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(into_thread))
}

/// List all threads in a session.
pub async fn list_threads(pool: &DbPool, session_id: &str) -> Result<Vec<Thread>> {
    let rows: Vec<ThreadRow> = sqlx::query_as(
        "SELECT id, session_id, role, provider, model, heartbeat_at, updated_at \
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
    let row: Option<ThreadRow> = sqlx::query_as(
        "SELECT id, session_id, role, provider, model, heartbeat_at, updated_at \
             FROM threads WHERE session_id = ? AND role = ? LIMIT 1",
    )
    .bind(session_id)
    .bind(role)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(into_thread))
}

/// Update the heartbeat timestamp on a thread. Call this periodically.
pub async fn heartbeat(pool: &DbPool, thread_id: &str) -> Result<()> {
    sqlx::query("UPDATE threads SET heartbeat_at = datetime('now') WHERE id = ?")
        .bind(thread_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Clear the heartbeat on a thread (detach / clean exit).
pub async fn clear_heartbeat(pool: &DbPool, thread_id: &str) -> Result<()> {
    sqlx::query("UPDATE threads SET heartbeat_at = NULL WHERE id = ?")
        .bind(thread_id)
        .execute(pool)
        .await?;
    Ok(())
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

fn into_thread(r: ThreadRow) -> Thread {
    Thread {
        id: r.0,
        session_id: r.1,
        role: r.2,
        provider: r.3,
        model: r.4,
        heartbeat_at: r.5,
        updated_at: r.6,
    }
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

/// Load all messages for a thread, ordered by ID.
pub async fn load_messages(pool: &DbPool, thread_id: &str) -> Result<Vec<StoredMessage>> {
    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT id, thread_id, role, content FROM messages WHERE thread_id = ? ORDER BY id",
    )
    .bind(thread_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(into_message).collect())
}

/// Load all messages across all threads in a session.
pub async fn load_session_messages(pool: &DbPool, session_id: &str) -> Result<Vec<StoredMessage>> {
    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT m.id, m.thread_id, m.role, m.content \
         FROM messages m JOIN threads t ON m.thread_id = t.id \
         WHERE t.session_id = ? ORDER BY m.id",
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
        assert!(Uuid::parse_str(&new_id()).is_ok());
    }

    #[test]
    fn new_id_is_v7() {
        let uuid = Uuid::parse_str(&new_id()).unwrap();
        assert_eq!(uuid.get_version(), Some(uuid::Version::SortRand));
    }

    #[test]
    fn new_ids_are_time_sorted() {
        let id1 = new_id();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let id2 = new_id();
        assert!(id1 < id2);
    }

    // --- Session tests ---

    #[tokio::test]
    async fn create_session_defaults_to_active() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, "test", "/tmp", None).await.unwrap();
        assert_eq!(s.status, SessionStatus::Active);
    }

    #[tokio::test]
    async fn archive_and_activate_session() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, "test", "/tmp", None).await.unwrap();

        archive_session(&pool, &s.id).await.unwrap();
        let s = get_session(&pool, &s.id).await.unwrap().unwrap();
        assert_eq!(s.status, SessionStatus::Archived);

        activate_session(&pool, &s.id).await.unwrap();
        let s = get_session(&pool, &s.id).await.unwrap().unwrap();
        assert_eq!(s.status, SessionStatus::Active);
    }

    #[tokio::test]
    async fn list_sessions_filter_by_status() {
        let pool = open_memory().await.unwrap();
        let s1 = create_session(&pool, "active-one", "/tmp/a", None)
            .await
            .unwrap();
        create_session(&pool, "active-two", "/tmp/b", None)
            .await
            .unwrap();
        archive_session(&pool, &s1.id).await.unwrap();

        let active = list_sessions(&pool, Some(SessionStatus::Active), 10)
            .await
            .unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].name, "active-two");

        let archived = list_sessions(&pool, Some(SessionStatus::Archived), 10)
            .await
            .unwrap();
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].name, "active-one");

        let all = list_sessions(&pool, None, 10).await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn get_session_by_name_works() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, "kodo-refactor", "/tmp", None)
            .await
            .unwrap();
        let found = get_session_by_name(&pool, "kodo-refactor")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.id, s.id);
        assert!(get_session_by_name(&pool, "nope").await.unwrap().is_none());
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

        assert_eq!(search_sessions(&pool, "kodo", 10).await.unwrap().len(), 2);
        assert_eq!(
            search_sessions(&pool, "phase-1", 10).await.unwrap().len(),
            1
        );
        assert!(search_sessions(&pool, "nope", 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn delete_session_cascades() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, "doomed", "/tmp", None).await.unwrap();
        let t = create_thread(&pool, &s.id, "default", "anthropic", "claude")
            .await
            .unwrap();
        save_message(&pool, &t.id, "user", r#"[{"type":"text","text":"hi"}]"#)
            .await
            .unwrap();

        delete_session(&pool, &s.id).await.unwrap();
        assert!(get_session(&pool, &s.id).await.unwrap().is_none());
        assert!(get_thread(&pool, &t.id).await.unwrap().is_none());
        assert!(load_messages(&pool, &t.id).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn delete_nonexistent_session_fails() {
        let pool = open_memory().await.unwrap();
        assert!(delete_session(&pool, "nope").await.is_err());
    }

    // --- Thread heartbeat tests ---

    #[tokio::test]
    async fn thread_starts_without_heartbeat() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, "test", "/tmp", None).await.unwrap();
        let t = create_thread(&pool, &s.id, "default", "anthropic", "claude")
            .await
            .unwrap();
        assert!(t.heartbeat_at.is_none());
        assert!(!t.is_live());
    }

    #[tokio::test]
    async fn heartbeat_makes_thread_live() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, "test", "/tmp", None).await.unwrap();
        let t = create_thread(&pool, &s.id, "default", "anthropic", "claude")
            .await
            .unwrap();

        heartbeat(&pool, &t.id).await.unwrap();
        let t = get_thread(&pool, &t.id).await.unwrap().unwrap();
        assert!(t.heartbeat_at.is_some());
        assert!(t.is_live());
    }

    #[tokio::test]
    async fn clear_heartbeat_makes_thread_not_live() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, "test", "/tmp", None).await.unwrap();
        let t = create_thread(&pool, &s.id, "default", "anthropic", "claude")
            .await
            .unwrap();

        heartbeat(&pool, &t.id).await.unwrap();
        clear_heartbeat(&pool, &t.id).await.unwrap();
        let t = get_thread(&pool, &t.id).await.unwrap().unwrap();
        assert!(!t.is_live());
    }

    // --- Sweep stale sessions ---

    #[tokio::test]
    async fn sweep_archives_sessions_with_no_live_threads() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, "stale", "/tmp", None).await.unwrap();
        create_thread(&pool, &s.id, "default", "anthropic", "claude")
            .await
            .unwrap();
        // No heartbeat set — thread is not live.

        let count = sweep_stale_sessions(&pool).await.unwrap();
        assert_eq!(count, 1);

        let s = get_session(&pool, &s.id).await.unwrap().unwrap();
        assert_eq!(s.status, SessionStatus::Archived);
    }

    #[tokio::test]
    async fn sweep_does_not_archive_sessions_with_live_threads() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, "alive", "/tmp", None).await.unwrap();
        let t = create_thread(&pool, &s.id, "default", "anthropic", "claude")
            .await
            .unwrap();
        heartbeat(&pool, &t.id).await.unwrap();

        let count = sweep_stale_sessions(&pool).await.unwrap();
        assert_eq!(count, 0);

        let s = get_session(&pool, &s.id).await.unwrap().unwrap();
        assert_eq!(s.status, SessionStatus::Active);
    }

    #[tokio::test]
    async fn sweep_skips_already_archived() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, "old", "/tmp", None).await.unwrap();
        create_thread(&pool, &s.id, "default", "anthropic", "claude")
            .await
            .unwrap();
        archive_session(&pool, &s.id).await.unwrap();

        let count = sweep_stale_sessions(&pool).await.unwrap();
        assert_eq!(count, 0); // Already archived, shouldn't be counted.
    }

    // --- Thread multi-model tests ---

    #[tokio::test]
    async fn multiple_threads_in_session() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, "multi", "/tmp", None).await.unwrap();

        create_thread(&pool, &s.id, "planner", "anthropic", "opus")
            .await
            .unwrap();
        create_thread(&pool, &s.id, "executor", "anthropic", "sonnet")
            .await
            .unwrap();
        create_thread(&pool, &s.id, "debugger", "openai", "gpt-4o")
            .await
            .unwrap();

        let threads = list_threads(&pool, &s.id).await.unwrap();
        assert_eq!(threads.len(), 3);
    }

    #[tokio::test]
    async fn get_thread_by_role_works() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, "test", "/tmp", None).await.unwrap();
        create_thread(&pool, &s.id, "planner", "anthropic", "opus")
            .await
            .unwrap();

        let planner = get_thread_by_role(&pool, &s.id, "planner")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(planner.model, "opus");
        assert!(
            get_thread_by_role(&pool, &s.id, "nope")
                .await
                .unwrap()
                .is_none()
        );
    }

    // --- Message tests ---

    #[tokio::test]
    async fn save_and_load_messages() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, "test", "/tmp", None).await.unwrap();
        let t = create_thread(&pool, &s.id, "default", "anthropic", "claude")
            .await
            .unwrap();

        let mid = save_message(&pool, &t.id, "user", r#"[{"type":"text","text":"hello"}]"#)
            .await
            .unwrap();
        assert!(Uuid::parse_str(&mid).is_ok());

        save_message(
            &pool,
            &t.id,
            "assistant",
            r#"[{"type":"text","text":"hi"}]"#,
        )
        .await
        .unwrap();

        let msgs = load_messages(&pool, &t.id).await.unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[1].role, "assistant");
    }

    #[tokio::test]
    async fn load_all_session_messages() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, "test", "/tmp", None).await.unwrap();
        let t1 = create_thread(&pool, &s.id, "planner", "anthropic", "opus")
            .await
            .unwrap();
        let t2 = create_thread(&pool, &s.id, "executor", "anthropic", "sonnet")
            .await
            .unwrap();

        save_message(&pool, &t1.id, "user", r#"[{"type":"text","text":"plan"}]"#)
            .await
            .unwrap();
        save_message(&pool, &t2.id, "user", r#"[{"type":"text","text":"exec"}]"#)
            .await
            .unwrap();

        let all = load_session_messages(&pool, &s.id).await.unwrap();
        assert_eq!(all.len(), 2);
    }

    // --- Fork tests ---

    #[tokio::test]
    async fn fork_session_copies_threads_and_messages() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, "original", "/tmp", Some("main"))
            .await
            .unwrap();
        let t1 = create_thread(&pool, &s.id, "planner", "anthropic", "opus")
            .await
            .unwrap();
        save_message(&pool, &t1.id, "user", r#"[{"type":"text","text":"msg"}]"#)
            .await
            .unwrap();

        let forked = fork_session(&pool, &s.id, "forked-copy").await.unwrap();
        assert_eq!(forked.name, "forked-copy");
        assert_eq!(forked.status, SessionStatus::Active);

        let threads = list_threads(&pool, &forked.id).await.unwrap();
        assert_eq!(threads.len(), 1);

        let msgs = load_session_messages(&pool, &forked.id).await.unwrap();
        assert_eq!(msgs.len(), 1);
    }
}
