-- Parents are captured when a local turn starts, before another device's continuation arrives.
-- Imported turns retain their source agent without fabricating a native agent session.
ALTER TABLE turns ADD COLUMN parent_turn_id TEXT;
ALTER TABLE turns ADD COLUMN source_agent TEXT;
UPDATE turns SET parent_turn_id = (SELECT p.id FROM turns p
    WHERE p.chat_id = turns.chat_id AND p.seq < turns.seq ORDER BY p.seq DESC LIMIT 1);
CREATE INDEX turns_parent ON turns (chat_id, parent_turn_id);

-- Remote heads may arrive before their chat. These rows intentionally have no chat/turn FK.
CREATE TABLE sync_turns (
    turn_id TEXT PRIMARY KEY,
    chat_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('capture', 'ready', 'remote', 'staged', 'imported')),
    cursor_json TEXT,
    tail_blob TEXT,
    part_count INTEGER NOT NULL DEFAULT 0 CHECK (part_count >= 0),
    attempt INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX sync_turns_state ON sync_turns (state, attempt, turn_id);
CREATE INDEX sync_turns_chat ON sync_turns (chat_id, state);
CREATE TABLE sync_turn_parts (
    turn_id TEXT NOT NULL REFERENCES sync_turns (turn_id),
    part_index INTEGER NOT NULL CHECK (part_index >= 0),
    blob_id TEXT NOT NULL UNIQUE,
    PRIMARY KEY (turn_id, part_index)
);
CREATE TABLE sync_turn_files (
    turn_id TEXT NOT NULL REFERENCES sync_turns (turn_id),
    hash TEXT NOT NULL,
    captured INTEGER NOT NULL DEFAULT 0 CHECK (captured >= 0),
    complete INTEGER NOT NULL DEFAULT 0 CHECK (complete IN (0, 1)),
    PRIMARY KEY (turn_id, hash)
);
-- A single compacted text row can expand beyond a wire piece when JSON escapes control characters.
-- The bounded remainder lives only in the encrypted main database between capture transactions.
CREATE TABLE sync_turn_fragments (
    turn_id TEXT PRIMARY KEY REFERENCES sync_turns (turn_id),
    bytes BLOB NOT NULL,
    captured INTEGER NOT NULL DEFAULT 0 CHECK (captured >= 0),
    next_cursor TEXT NOT NULL
);
