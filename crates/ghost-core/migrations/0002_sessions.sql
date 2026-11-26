-- Schema v2: what M1a's session manager needs on top of v1.
-- Only ADD COLUMN and new objects: nothing is rebuilt, existing rows keep their values (new columns are NULL).

-- The user's explicit option choices, re-applied when the agent starts: JSON {option_id: ConfigValue}.
ALTER TABLE agent_sessions ADD COLUMN config_json TEXT;

-- The prompt a turn came from, the seq range its events occupy, and when it was compacted.
ALTER TABLE turns ADD COLUMN prompt_id TEXT;
ALTER TABLE turns ADD COLUMN first_event_seq INTEGER;
ALTER TABLE turns ADD COLUMN last_event_seq INTEGER;
ALTER TABLE turns ADD COLUMN compacted_at INTEGER;
-- History pages walk turns by their first seq; recovery reads only unfinished or uncompacted turns.
CREATE INDEX turns_chat_first_event_seq ON turns (chat_id, first_event_seq);
CREATE INDEX turns_running ON turns (chat_id) WHERE status = 'running';
CREATE INDEX turns_uncompacted ON turns (chat_id) WHERE status <> 'running' AND compacted_at IS NULL;

-- Permission requests, questions and their resolutions: recovery finds unanswered ones without scanning events.
CREATE INDEX events_requests ON events (chat_id, seq)
    WHERE kind IN ('permission_requested', 'permission_resolved', 'ui_request', 'ui_resolved');

-- Device-local and never synced: prompts waiting behind a running turn, so a crash can't lose them.
-- A row is removed when its prompt starts (in the BeginTurn transaction) or is cancelled.
-- held = 1: queued before a stop, error, crash or restart; sent only after the user says so.
CREATE TABLE queued_prompts (
    id TEXT PRIMARY KEY,
    chat_id TEXT NOT NULL REFERENCES chats (id),
    text TEXT NOT NULL,
    held INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    device_id TEXT NOT NULL
);
CREATE INDEX queued_prompts_chat_id ON queued_prompts (chat_id, created_at);
