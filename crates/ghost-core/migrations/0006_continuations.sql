-- Visible context and startup choices are saved before a continuation can reach an agent.
-- The source remains intact; a fork links back to it instead of duplicating its transcript.
ALTER TABLE turns ADD COLUMN continuation_prefix TEXT;
ALTER TABLE agent_sessions ADD COLUMN capabilities_json TEXT;
CREATE TABLE chat_continuations (
    chat_id TEXT PRIMARY KEY REFERENCES chats (id),
    source_chat_id TEXT NOT NULL REFERENCES chats (id),
    source_turn_id TEXT REFERENCES turns (id),
    pending_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    device_id TEXT NOT NULL,
    deleted INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX chat_continuations_source_chat ON chat_continuations (source_chat_id);
CREATE INDEX chat_continuations_source_turn ON chat_continuations (source_turn_id);
