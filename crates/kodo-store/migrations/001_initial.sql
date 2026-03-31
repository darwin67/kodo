-- All IDs are UUIDv7 (time-sortable, embeds creation timestamp).
-- No separate created_at column needed — extract from the UUID if required.

-- Sessions: shared context space for a project directory.
CREATE TABLE IF NOT EXISTS sessions (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    directory   TEXT NOT NULL,
    branch      TEXT,
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_sessions_name ON sessions(name);

-- Threads: a conversation track within a session.
CREATE TABLE IF NOT EXISTS threads (
    id          TEXT PRIMARY KEY,
    session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    role        TEXT NOT NULL DEFAULT 'default',
    provider    TEXT NOT NULL,
    model       TEXT NOT NULL,
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_threads_session ON threads(session_id);

-- Messages: conversation messages within a thread.
CREATE TABLE IF NOT EXISTS messages (
    id          TEXT PRIMARY KEY,
    thread_id   TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    role        TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
    content     TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_messages_thread ON messages(thread_id, id);

-- Auth providers: tracks which providers have credentials in OS keychain.
CREATE TABLE IF NOT EXISTS auth_providers (
    provider    TEXT PRIMARY KEY,
    expires_at  TEXT,
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Memory: key-value store for project/global memory.
CREATE TABLE IF NOT EXISTS memory (
    id         TEXT PRIMARY KEY,
    key        TEXT NOT NULL,
    value      TEXT NOT NULL,
    scope      TEXT NOT NULL CHECK (scope IN ('global', 'project')),
    project    TEXT,
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(key, scope, project)
);
