-- Sessions: shared context space for a project directory.
-- A session can contain multiple threads, each using a different model.
CREATE TABLE IF NOT EXISTS sessions (
    id          TEXT PRIMARY KEY,
    directory   TEXT NOT NULL,
    branch      TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Threads: a conversation track within a session.
-- Each thread has its own provider/model and a role describing its purpose.
-- Multiple threads can coexist in a single session, sharing context.
CREATE TABLE IF NOT EXISTS threads (
    id          TEXT PRIMARY KEY,
    session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    role        TEXT NOT NULL DEFAULT 'default',
    provider    TEXT NOT NULL,
    model       TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_threads_session ON threads(session_id);

-- Messages: conversation messages within a thread.
CREATE TABLE IF NOT EXISTS messages (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    thread_id   TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    role        TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
    content     TEXT NOT NULL,  -- JSON array of ContentBlocks
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_messages_thread ON messages(thread_id, id);

-- Auth tokens: stores provider credentials (secrets in OS keychain).
CREATE TABLE IF NOT EXISTS auth_tokens (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    provider       TEXT NOT NULL UNIQUE,
    token          TEXT NOT NULL,
    refresh_token  TEXT,
    expires_at     TEXT,
    created_at     TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at     TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Memory: key-value store for project/global memory.
CREATE TABLE IF NOT EXISTS memory (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    key        TEXT NOT NULL,
    value      TEXT NOT NULL,
    scope      TEXT NOT NULL CHECK (scope IN ('global', 'project')),
    project    TEXT,  -- project directory path, NULL for global scope
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(key, scope, project)
);
