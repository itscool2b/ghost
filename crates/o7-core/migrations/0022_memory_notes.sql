-- Project memory becomes a short brief that every chat of the project receives, and notes that are
-- listed to every chat and read on demand. `project_memory` holds the brief; its `revision` is now
-- the newest revision of anything in the project's memory, the brief or a note. A revision is of
-- the brief (`note` is NULL) or of one note, and one chain of them covers the project.
CREATE TABLE project_memory_notes (
 project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
 name TEXT NOT NULL,
 description TEXT NOT NULL,
 revision TEXT NOT NULL,
 markdown TEXT NOT NULL,
 source TEXT NOT NULL,
 updated_at INTEGER NOT NULL,
 PRIMARY KEY (project_id, name)
);
ALTER TABLE project_memory_revisions ADD COLUMN note TEXT;
ALTER TABLE project_memory_revisions ADD COLUMN description TEXT;
ALTER TABLE project_memory_revisions ADD COLUMN deleted INTEGER NOT NULL DEFAULT 0;
CREATE INDEX project_memory_note_history ON project_memory_revisions(project_id, note);
-- The indexes startup recovery reads through. A database from before them has them built after
-- startup (chatstore/recover.rs); one created now starts with them.
CREATE INDEX IF NOT EXISTS events_steering ON events (chat_id, seq) WHERE kind = 'steering_status';
CREATE INDEX IF NOT EXISTS events_turn_ended ON events (turn_id, seq) WHERE kind = 'turn_ended';
