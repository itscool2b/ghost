-- All content stays on the profile writer connection. Account profiles inherit SQLCipher.
CREATE TABLE project_memory (
 project_id TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
 revision TEXT NOT NULL,
 markdown TEXT NOT NULL,
 updated_at INTEGER NOT NULL
);
CREATE TABLE project_memory_revisions (
 revision TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
 parent_revision TEXT,
 markdown TEXT NOT NULL,
 reason TEXT NOT NULL,
 source TEXT NOT NULL,
 created_at INTEGER NOT NULL
);
CREATE INDEX project_memory_history ON project_memory_revisions(project_id,created_at);
CREATE TABLE project_memory_proposals (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
 base_revision TEXT,
 markdown TEXT NOT NULL,
 reason TEXT NOT NULL,
 chat_id TEXT,
 idempotency_key TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN ('pending','accepted','rejected','conflict')),
 created_at INTEGER NOT NULL,
 UNIQUE(project_id,idempotency_key)
);
CREATE TABLE project_memory_snapshots (
 prompt_id TEXT PRIMARY KEY,
 chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
 project_id TEXT NOT NULL,
 revision TEXT,
 markdown TEXT NOT NULL,
 visible_text TEXT NOT NULL,
 created_at INTEGER NOT NULL
);
