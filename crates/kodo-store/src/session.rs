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

/// Recommended heartbeat interval.
pub const HEARTBEAT_INTERVAL_SECS: u64 = 20;

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
    pub name: String,
    pub role: String,
    pub provider: String,
    pub model: String,
    pub heartbeat_at: Option<String>,
    pub updated_at: String,
}

impl Thread {
    /// Whether this thread has a live heartbeat (not stale).
    pub fn is_live(&self) -> bool {
        match &self.heartbeat_at {
            None => false,
            Some(hb) => chrono::NaiveDateTime::parse_from_str(hb, "%Y-%m-%d %H:%M:%S")
                .map(|dt| {
                    let now = chrono::Utc::now().naive_utc();
                    (now - dt).num_seconds() < HEARTBEAT_STALE_SECS
                })
                .unwrap_or(false),
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

// Row type aliases for query_as (avoids unwieldy inline tuples).
type SessionRow = (String, String, String, Option<String>, String, String);
type ThreadRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    String,
);

fn into_session(r: SessionRow) -> Session {
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

fn into_thread(r: ThreadRow) -> Thread {
    Thread {
        id: r.0,
        session_id: r.1,
        name: r.2,
        role: r.3,
        provider: r.4,
        model: r.5,
        heartbeat_at: r.6,
        updated_at: r.7,
    }
}

fn into_message(r: (String, String, String, String)) -> StoredMessage {
    StoredMessage {
        id: r.0,
        thread_id: r.1,
        role: r.2,
        content: r.3,
    }
}

const SESSION_COLS: &str = "id, name, directory, branch, status, updated_at";
const THREAD_COLS: &str = "id, session_id, name, role, provider, model, heartbeat_at, updated_at";

// ---------------------------------------------------------------------------
// Session CRUD
// ---------------------------------------------------------------------------

/// Create a new session.
/// If `name` is None, a placeholder "Untitled" is used. The caller can later
/// rename it (e.g. after the first user message, using a short summary).
pub async fn create_session(
    pool: &DbPool,
    name: Option<&str>,
    directory: &str,
    branch: Option<&str>,
) -> Result<Session> {
    let id = new_id();
    let name = name.unwrap_or("Untitled");
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
    let row: Option<SessionRow> =
        sqlx::query_as(&format!("SELECT {SESSION_COLS} FROM sessions WHERE id = ?"))
            .bind(id)
            .fetch_optional(pool)
            .await?;

    Ok(row.map(into_session))
}

/// Find a session by name (exact match).
pub async fn get_session_by_name(pool: &DbPool, name: &str) -> Result<Option<Session>> {
    let row: Option<SessionRow> = sqlx::query_as(&format!(
        "SELECT {SESSION_COLS} FROM sessions WHERE name = ? LIMIT 1"
    ))
    .bind(name)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(into_session))
}

/// Search sessions by name substring (case-insensitive).
pub async fn search_sessions(pool: &DbPool, query: &str, limit: u32) -> Result<Vec<Session>> {
    let pattern = format!("%{query}%");
    let rows: Vec<SessionRow> = sqlx::query_as(&format!(
        "SELECT {SESSION_COLS} FROM sessions WHERE name LIKE ? ORDER BY updated_at DESC LIMIT ?"
    ))
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
    let rows: Vec<SessionRow> = match status {
        Some(s) => sqlx::query_as(&format!(
            "SELECT {SESSION_COLS} FROM sessions WHERE status = ? ORDER BY updated_at DESC LIMIT ?"
        ))
        .bind(s.to_string())
        .bind(limit)
        .fetch_all(pool)
        .await?,
        None => {
            sqlx::query_as(&format!(
                "SELECT {SESSION_COLS} FROM sessions ORDER BY updated_at DESC LIMIT ?"
            ))
            .bind(limit)
            .fetch_all(pool)
            .await?
        }
    };

    Ok(rows.into_iter().map(into_session).collect())
}

/// Rename a session.
pub async fn rename_session(pool: &DbPool, id: &str, new_name: &str) -> Result<()> {
    let result =
        sqlx::query("UPDATE sessions SET name = ?, updated_at = datetime('now') WHERE id = ?")
            .bind(new_name)
            .bind(id)
            .execute(pool)
            .await?;

    if result.rows_affected() == 0 {
        bail!("session not found: {id}");
    }
    Ok(())
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
pub async fn fork_session(
    pool: &DbPool,
    source_id: &str,
    new_name: Option<&str>,
) -> Result<Session> {
    let source = get_session(pool, source_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("source session not found: {source_id}"))?;

    let fork_name = new_name.unwrap_or("Untitled");
    let new_session = create_session(
        pool,
        Some(fork_name),
        &source.directory,
        source.branch.as_deref(),
    )
    .await?;

    let source_threads = list_threads(pool, source_id).await?;
    for thread in &source_threads {
        let new_thread = create_thread(
            pool,
            &new_session.id,
            Some(&thread.name),
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

/// Sweep stale sessions: archive active sessions with no live threads.
pub async fn sweep_stale_sessions(pool: &DbPool) -> Result<u32> {
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

// ---------------------------------------------------------------------------
// Thread CRUD
// ---------------------------------------------------------------------------

/// Count threads in a session (used for auto-naming).
async fn thread_count(pool: &DbPool, session_id: &str) -> Result<i64> {
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM threads WHERE session_id = ?")
        .bind(session_id)
        .fetch_one(pool)
        .await?;
    Ok(count)
}

/// Create a new thread within a session.
/// If `name` is None, auto-generates "Thread N" where N is the next number.
pub async fn create_thread(
    pool: &DbPool,
    session_id: &str,
    name: Option<&str>,
    role: &str,
    provider: &str,
    model: &str,
) -> Result<Thread> {
    let id = new_id();

    let thread_name = match name {
        Some(n) => n.to_string(),
        None => {
            let count = thread_count(pool, session_id).await?;
            format!("Thread {}", count + 1)
        }
    };

    debug!(id = %id, session_id, name = %thread_name, role, provider, model, "creating thread");

    sqlx::query(
        "INSERT INTO threads (id, session_id, name, role, provider, model) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(session_id)
    .bind(&thread_name)
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
    let row: Option<ThreadRow> =
        sqlx::query_as(&format!("SELECT {THREAD_COLS} FROM threads WHERE id = ?"))
            .bind(id)
            .fetch_optional(pool)
            .await?;

    Ok(row.map(into_thread))
}

/// List all threads in a session.
pub async fn list_threads(pool: &DbPool, session_id: &str) -> Result<Vec<Thread>> {
    let rows: Vec<ThreadRow> = sqlx::query_as(&format!(
        "SELECT {THREAD_COLS} FROM threads WHERE session_id = ? ORDER BY id"
    ))
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
    let row: Option<ThreadRow> = sqlx::query_as(&format!(
        "SELECT {THREAD_COLS} FROM threads WHERE session_id = ? AND role = ? LIMIT 1"
    ))
    .bind(session_id)
    .bind(role)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(into_thread))
}

/// Rename a thread.
pub async fn rename_thread(pool: &DbPool, id: &str, new_name: &str) -> Result<()> {
    let result =
        sqlx::query("UPDATE threads SET name = ?, updated_at = datetime('now') WHERE id = ?")
            .bind(new_name)
            .bind(id)
            .execute(pool)
            .await?;

    if result.rows_affected() == 0 {
        bail!("thread not found: {id}");
    }
    Ok(())
}

/// Update the heartbeat timestamp on a thread.
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_memory;

    // --- ID ---

    #[test]
    fn new_id_is_valid_v7_uuid() {
        let id = new_id();
        let uuid = Uuid::parse_str(&id).unwrap();
        assert_eq!(uuid.get_version(), Some(uuid::Version::SortRand));
    }

    #[test]
    fn new_ids_are_time_sorted() {
        let id1 = new_id();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let id2 = new_id();
        assert!(id1 < id2);
    }

    // --- Session naming ---

    #[tokio::test]
    async fn session_with_explicit_name() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, Some("my-refactor"), "/tmp", None)
            .await
            .unwrap();
        assert_eq!(s.name, "my-refactor");
    }

    #[tokio::test]
    async fn session_without_name_gets_untitled() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, None, "/tmp", None).await.unwrap();
        assert_eq!(s.name, "Untitled");
    }

    #[tokio::test]
    async fn rename_session_works() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, None, "/tmp", None).await.unwrap();
        assert_eq!(s.name, "Untitled");

        rename_session(&pool, &s.id, "refactor-auth").await.unwrap();
        let s = get_session(&pool, &s.id).await.unwrap().unwrap();
        assert_eq!(s.name, "refactor-auth");
    }

    #[tokio::test]
    async fn rename_nonexistent_session_fails() {
        let pool = open_memory().await.unwrap();
        assert!(rename_session(&pool, "nope", "new-name").await.is_err());
    }

    // --- Session search ---

    #[tokio::test]
    async fn get_session_by_name_works() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, Some("kodo-refactor"), "/tmp", None)
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
        create_session(&pool, Some("kodo-phase-1"), "/tmp/a", None)
            .await
            .unwrap();
        create_session(&pool, Some("kodo-phase-2"), "/tmp/b", None)
            .await
            .unwrap();
        create_session(&pool, Some("other-project"), "/tmp/c", None)
            .await
            .unwrap();

        assert_eq!(search_sessions(&pool, "kodo", 10).await.unwrap().len(), 2);
        assert_eq!(
            search_sessions(&pool, "phase-1", 10).await.unwrap().len(),
            1
        );
        assert!(search_sessions(&pool, "nope", 10).await.unwrap().is_empty());
    }

    // --- Session status ---

    #[tokio::test]
    async fn session_defaults_to_active() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, Some("test"), "/tmp", None)
            .await
            .unwrap();
        assert_eq!(s.status, SessionStatus::Active);
    }

    #[tokio::test]
    async fn archive_and_activate_session() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, Some("test"), "/tmp", None)
            .await
            .unwrap();

        archive_session(&pool, &s.id).await.unwrap();
        assert_eq!(
            get_session(&pool, &s.id).await.unwrap().unwrap().status,
            SessionStatus::Archived
        );

        activate_session(&pool, &s.id).await.unwrap();
        assert_eq!(
            get_session(&pool, &s.id).await.unwrap().unwrap().status,
            SessionStatus::Active
        );
    }

    #[tokio::test]
    async fn list_sessions_filter_by_status() {
        let pool = open_memory().await.unwrap();
        let s1 = create_session(&pool, Some("one"), "/tmp/a", None)
            .await
            .unwrap();
        create_session(&pool, Some("two"), "/tmp/b", None)
            .await
            .unwrap();
        archive_session(&pool, &s1.id).await.unwrap();

        assert_eq!(
            list_sessions(&pool, Some(SessionStatus::Active), 10)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            list_sessions(&pool, Some(SessionStatus::Archived), 10)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(list_sessions(&pool, None, 10).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn delete_session_cascades() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, Some("doomed"), "/tmp", None)
            .await
            .unwrap();
        let t = create_thread(&pool, &s.id, None, "default", "anthropic", "claude")
            .await
            .unwrap();
        save_message(&pool, &t.id, "user", r#"[{"type":"text","text":"hi"}]"#)
            .await
            .unwrap();

        delete_session(&pool, &s.id).await.unwrap();
        assert!(get_session(&pool, &s.id).await.unwrap().is_none());
        assert!(get_thread(&pool, &t.id).await.unwrap().is_none());
    }

    // --- Thread naming ---

    #[tokio::test]
    async fn thread_with_explicit_name() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, Some("test"), "/tmp", None)
            .await
            .unwrap();
        let t = create_thread(
            &pool,
            &s.id,
            Some("planner-opus"),
            "planner",
            "anthropic",
            "opus",
        )
        .await
        .unwrap();
        assert_eq!(t.name, "planner-opus");
    }

    #[tokio::test]
    async fn thread_auto_names_incrementing() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, Some("test"), "/tmp", None)
            .await
            .unwrap();

        let t1 = create_thread(&pool, &s.id, None, "default", "anthropic", "claude")
            .await
            .unwrap();
        let t2 = create_thread(&pool, &s.id, None, "executor", "openai", "gpt-4o")
            .await
            .unwrap();
        let t3 = create_thread(&pool, &s.id, None, "debugger", "anthropic", "sonnet")
            .await
            .unwrap();

        assert_eq!(t1.name, "Thread 1");
        assert_eq!(t2.name, "Thread 2");
        assert_eq!(t3.name, "Thread 3");
    }

    #[tokio::test]
    async fn rename_thread_works() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, Some("test"), "/tmp", None)
            .await
            .unwrap();
        let t = create_thread(&pool, &s.id, None, "default", "anthropic", "claude")
            .await
            .unwrap();
        assert_eq!(t.name, "Thread 1");

        rename_thread(&pool, &t.id, "main-chat").await.unwrap();
        let t = get_thread(&pool, &t.id).await.unwrap().unwrap();
        assert_eq!(t.name, "main-chat");
    }

    #[tokio::test]
    async fn rename_nonexistent_thread_fails() {
        let pool = open_memory().await.unwrap();
        assert!(rename_thread(&pool, "nope", "new-name").await.is_err());
    }

    // --- Thread heartbeat ---

    #[tokio::test]
    async fn thread_starts_without_heartbeat() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, Some("test"), "/tmp", None)
            .await
            .unwrap();
        let t = create_thread(&pool, &s.id, None, "default", "anthropic", "claude")
            .await
            .unwrap();
        assert!(!t.is_live());
    }

    #[tokio::test]
    async fn heartbeat_makes_thread_live() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, Some("test"), "/tmp", None)
            .await
            .unwrap();
        let t = create_thread(&pool, &s.id, None, "default", "anthropic", "claude")
            .await
            .unwrap();

        heartbeat(&pool, &t.id).await.unwrap();
        let t = get_thread(&pool, &t.id).await.unwrap().unwrap();
        assert!(t.is_live());
    }

    #[tokio::test]
    async fn clear_heartbeat_detaches() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, Some("test"), "/tmp", None)
            .await
            .unwrap();
        let t = create_thread(&pool, &s.id, None, "default", "anthropic", "claude")
            .await
            .unwrap();

        heartbeat(&pool, &t.id).await.unwrap();
        clear_heartbeat(&pool, &t.id).await.unwrap();
        assert!(!get_thread(&pool, &t.id).await.unwrap().unwrap().is_live());
    }

    // --- Sweep ---

    #[tokio::test]
    async fn sweep_archives_stale_sessions() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, Some("stale"), "/tmp", None)
            .await
            .unwrap();
        create_thread(&pool, &s.id, None, "default", "anthropic", "claude")
            .await
            .unwrap();

        assert_eq!(sweep_stale_sessions(&pool).await.unwrap(), 1);
        assert_eq!(
            get_session(&pool, &s.id).await.unwrap().unwrap().status,
            SessionStatus::Archived
        );
    }

    #[tokio::test]
    async fn sweep_skips_live_sessions() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, Some("alive"), "/tmp", None)
            .await
            .unwrap();
        let t = create_thread(&pool, &s.id, None, "default", "anthropic", "claude")
            .await
            .unwrap();
        heartbeat(&pool, &t.id).await.unwrap();

        assert_eq!(sweep_stale_sessions(&pool).await.unwrap(), 0);
        assert_eq!(
            get_session(&pool, &s.id).await.unwrap().unwrap().status,
            SessionStatus::Active
        );
    }

    // --- Multi-model threads ---

    #[tokio::test]
    async fn multiple_threads_different_models() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, Some("multi"), "/tmp", None)
            .await
            .unwrap();

        create_thread(
            &pool,
            &s.id,
            Some("Planner"),
            "planner",
            "anthropic",
            "opus",
        )
        .await
        .unwrap();
        create_thread(
            &pool,
            &s.id,
            Some("Coder"),
            "executor",
            "anthropic",
            "sonnet",
        )
        .await
        .unwrap();
        create_thread(
            &pool,
            &s.id,
            Some("Reviewer"),
            "debugger",
            "openai",
            "gpt-4o",
        )
        .await
        .unwrap();

        let threads = list_threads(&pool, &s.id).await.unwrap();
        assert_eq!(threads.len(), 3);

        let names: Vec<&str> = threads.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"Planner"));
        assert!(names.contains(&"Coder"));
        assert!(names.contains(&"Reviewer"));
    }

    #[tokio::test]
    async fn get_thread_by_role_works() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, Some("test"), "/tmp", None)
            .await
            .unwrap();
        create_thread(&pool, &s.id, None, "planner", "anthropic", "opus")
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

    // --- Messages ---

    #[tokio::test]
    async fn save_and_load_messages() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, Some("test"), "/tmp", None)
            .await
            .unwrap();
        let t = create_thread(&pool, &s.id, None, "default", "anthropic", "claude")
            .await
            .unwrap();

        save_message(&pool, &t.id, "user", r#"[{"type":"text","text":"hello"}]"#)
            .await
            .unwrap();
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
    }

    #[tokio::test]
    async fn load_session_messages_cross_thread() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, Some("test"), "/tmp", None)
            .await
            .unwrap();
        let t1 = create_thread(&pool, &s.id, None, "planner", "anthropic", "opus")
            .await
            .unwrap();
        let t2 = create_thread(&pool, &s.id, None, "executor", "anthropic", "sonnet")
            .await
            .unwrap();

        save_message(&pool, &t1.id, "user", r#"[{"type":"text","text":"plan"}]"#)
            .await
            .unwrap();
        save_message(&pool, &t2.id, "user", r#"[{"type":"text","text":"exec"}]"#)
            .await
            .unwrap();

        assert_eq!(load_session_messages(&pool, &s.id).await.unwrap().len(), 2);
    }

    // --- Fork ---

    #[tokio::test]
    async fn fork_copies_thread_names_and_messages() {
        let pool = open_memory().await.unwrap();
        let s = create_session(&pool, Some("original"), "/tmp", Some("main"))
            .await
            .unwrap();
        let t = create_thread(
            &pool,
            &s.id,
            Some("Planner"),
            "planner",
            "anthropic",
            "opus",
        )
        .await
        .unwrap();
        save_message(&pool, &t.id, "user", r#"[{"type":"text","text":"msg"}]"#)
            .await
            .unwrap();

        let forked = fork_session(&pool, &s.id, Some("forked")).await.unwrap();
        assert_eq!(forked.name, "forked");

        let threads = list_threads(&pool, &forked.id).await.unwrap();
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].name, "Planner"); // Name preserved from source.

        assert_eq!(
            load_session_messages(&pool, &forked.id)
                .await
                .unwrap()
                .len(),
            1
        );
    }
}
