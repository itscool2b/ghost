-- Schema v1: the local store for one profile.
--
-- Conventions (every later migration keeps them):
-- * TEXT ids are lowercase UUIDv7 strings (ghost_core::ids::new_id), times are INTEGER Unix ms.
-- * Rows that sync carry the device_id that last wrote them and a `deleted` tombstone flag. User data
--   is never hard-deleted, so a delete on one device can be replayed on another.
-- * Every foreign key column is the leading column of some index (explicit, primary key, or UNIQUE),
--   so parent lookups and FK checks never scan a table. ghost-core's tests verify this.
-- * Migrations run with foreign_keys=OFF inside one transaction and end with a foreign_key_check,
--   which is what lets a later migration rebuild a table (SQLite's documented ALTER TABLE procedure).

CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE projects (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    color TEXT,
    icon TEXT,
    default_agent TEXT,
    default_model TEXT,
    sort_key TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    device_id TEXT NOT NULL,
    deleted INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX projects_updated_at ON projects (updated_at);

-- A folder's portable identity (git remote + default branch, or local-only on its origin device).
-- Where it lives on each machine is in folder_locations, which is what lets projects sync across OSes.
CREATE TABLE project_folders (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects (id),
    display_name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('git', 'local')),
    git_remote_url TEXT,
    git_default_branch TEXT,
    origin_device_id TEXT,
    sort_key TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    device_id TEXT NOT NULL,
    deleted INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX project_folders_project_id ON project_folders (project_id);

-- Device-local and never synced: the path of a folder on one device.
-- The primary key leads with folder_id, which covers that foreign key.
CREATE TABLE folder_locations (
    folder_id TEXT NOT NULL REFERENCES project_folders (id),
    device_id TEXT NOT NULL,
    path TEXT NOT NULL,
    last_seen_at INTEGER,
    PRIMARY KEY (folder_id, device_id)
);

CREATE TABLE chats (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects (id),
    folder_id TEXT REFERENCES project_folders (id),
    title TEXT,
    title_source TEXT,
    active_agent TEXT,
    pinned INTEGER NOT NULL DEFAULT 0,
    archived INTEGER NOT NULL DEFAULT 0,
    isolation TEXT NOT NULL DEFAULT 'direct' CHECK (isolation IN ('direct', 'isolated')),
    worktree_branch TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    device_id TEXT NOT NULL,
    deleted INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX chats_project_id ON chats (project_id);
CREATE INDEX chats_folder_id ON chats (folder_id);
CREATE INDEX chats_updated_at ON chats (updated_at);

-- One native session of one agent inside a chat (a chat can span several agents).
CREATE TABLE agent_sessions (
    id TEXT PRIMARY KEY,
    chat_id TEXT NOT NULL REFERENCES chats (id),
    agent_id TEXT NOT NULL,
    host TEXT NOT NULL DEFAULT 'native',
    native_session_id TEXT,
    model TEXT,
    effort TEXT,
    autonomy TEXT,
    cwd TEXT NOT NULL,
    last_event_seq INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    device_id TEXT NOT NULL
);
CREATE INDEX agent_sessions_chat_id ON agent_sessions (chat_id);

-- UNIQUE (chat_id, seq) leads with chat_id, which covers that foreign key.
CREATE TABLE turns (
    id TEXT PRIMARY KEY,
    chat_id TEXT NOT NULL REFERENCES chats (id),
    agent_session_id TEXT REFERENCES agent_sessions (id),
    seq INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('running', 'completed', 'interrupted', 'failed')),
    stop_reason TEXT,
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    checkpoint_ref TEXT,
    device_id TEXT NOT NULL,
    UNIQUE (chat_id, seq)
);
CREATE INDEX turns_agent_session_id ON turns (agent_session_id);

-- Append-only GhostEvents (payload is the event's JSON), compacted per turn.
-- UNIQUE (chat_id, seq) leads with chat_id, which covers that foreign key.
CREATE TABLE events (
    id TEXT PRIMARY KEY,
    chat_id TEXT NOT NULL REFERENCES chats (id),
    turn_id TEXT REFERENCES turns (id),
    seq INTEGER NOT NULL,
    kind TEXT NOT NULL,
    payload TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    device_id TEXT NOT NULL,
    UNIQUE (chat_id, seq)
);
CREATE INDEX events_turn_id ON events (turn_id);

-- Content-addressed: the file is <profile>/attachments/<sha256>.
CREATE TABLE attachments (
    sha256 TEXT PRIMARY KEY,
    mime TEXT NOT NULL,
    size INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);

-- Device-local cache of agent detection and latest-version lookups.
CREATE TABLE agent_cache (
    agent_id TEXT PRIMARY KEY,
    detection TEXT,
    latest_version TEXT,
    checked_at INTEGER
);

-- Full-text search over messages, filled when a turn is compacted. remove_diacritics 2 folds accents
-- (searching "creme" finds "crème") including the code points older tokenizers missed.
CREATE VIRTUAL TABLE message_fts USING fts5 (
    chat_id UNINDEXED,
    message_id UNINDEXED,
    text,
    tokenize = 'unicode61 remove_diacritics 2'
);
