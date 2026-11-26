-- Coordinated projects (an experiment): a project's coordinator chat, its workers, the Context
-- files they share, the task board, the decision log and routines. All of it stays on the profile
-- writer connection and on this device; none of it syncs. Account profiles inherit SQLCipher.
CREATE TABLE coordinator_projects (
 project_id TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
 coordinator_chat_id TEXT,
 brief TEXT NOT NULL DEFAULT '',
 paused INTEGER NOT NULL DEFAULT 0,
 pause_reason TEXT,
 next_task INTEGER NOT NULL DEFAULT 1,
 next_decision INTEGER NOT NULL DEFAULT 1,
 created_at INTEGER NOT NULL,
 updated_at INTEGER NOT NULL
);
CREATE TABLE coordinator_workers (
 chat_id TEXT PRIMARY KEY REFERENCES chats(id) ON DELETE CASCADE,
 project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
 name TEXT NOT NULL,
 task_number INTEGER,
 -- What the worker filed during the turn under way, until that turn ends.
 filed TEXT,
 -- What its last finished turn reported.
 report TEXT,
 turns INTEGER NOT NULL DEFAULT 0,
 -- The turn count whose report reached the coordinator.
 delivered INTEGER NOT NULL DEFAULT 0,
 closed INTEGER NOT NULL DEFAULT 0,
 created_at INTEGER NOT NULL,
 reported_at INTEGER,
 UNIQUE(project_id,name)
);
CREATE INDEX coordinator_workers_project ON coordinator_workers(project_id,created_at);
CREATE TABLE coordinator_tasks (
 project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
 number INTEGER NOT NULL,
 title TEXT NOT NULL,
 body TEXT NOT NULL DEFAULT '',
 status TEXT NOT NULL CHECK(status IN ('todo','doing','review','done')),
 position INTEGER NOT NULL,
 tags TEXT NOT NULL DEFAULT '[]',
 worker TEXT,
 link TEXT,
 source TEXT NOT NULL,
 created_at INTEGER NOT NULL,
 updated_at INTEGER NOT NULL,
 PRIMARY KEY(project_id,number)
);
CREATE TABLE coordinator_files (
 project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
 path TEXT NOT NULL,
 revision TEXT NOT NULL,
 content TEXT NOT NULL,
 source TEXT NOT NULL,
 updated_at INTEGER NOT NULL,
 PRIMARY KEY(project_id,path)
);
-- Every write keeps what it replaced. `content` is NULL for a deletion.
CREATE TABLE coordinator_file_revisions (
 revision TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
 path TEXT NOT NULL,
 parent_revision TEXT,
 content TEXT,
 source TEXT NOT NULL,
 created_at INTEGER NOT NULL
);
CREATE INDEX coordinator_file_history ON coordinator_file_revisions(project_id,path,created_at);
-- Only ever added to. The person can retract an entry: it stays in the log, struck out, and
-- agents are no longer shown it.
CREATE TABLE coordinator_decisions (
 project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
 number INTEGER NOT NULL,
 text TEXT NOT NULL,
 source TEXT NOT NULL,
 created_at INTEGER NOT NULL,
 retracted_at INTEGER,
 PRIMARY KEY(project_id,number)
);
CREATE TABLE coordinator_routines (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
 title TEXT NOT NULL,
 prompt TEXT NOT NULL,
 every_minutes INTEGER NOT NULL,
 enabled INTEGER NOT NULL DEFAULT 1,
 next_at INTEGER NOT NULL,
 last_at INTEGER,
 created_at INTEGER NOT NULL
);
CREATE INDEX coordinator_routines_due ON coordinator_routines(enabled,next_at);
