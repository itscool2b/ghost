-- The Dashboard's activity calendar (owner, 20 November 2026): how many lines agents' edit tools
-- changed, per turn and quarter hour. Counts alone: no paths and no text. Derived from the turn's
-- own events on this device, so it is never synced; a lost row is counted again from them.
CREATE TABLE line_edits (
 turn_id TEXT NOT NULL REFERENCES turns(id) ON DELETE CASCADE,
 chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
 agent TEXT NOT NULL,
 -- When the quarter hour began, in Unix milliseconds: every time zone's day starts on one.
 at INTEGER NOT NULL,
 lines INTEGER NOT NULL CHECK(lines > 0),
 PRIMARY KEY(turn_id, at)
);
CREATE INDEX line_edits_at ON line_edits(at);

-- When a turn's edits were counted. An ended turn waits at NULL until the Dashboard next asks.
ALTER TABLE turns ADD COLUMN edits_counted_at INTEGER;
CREATE INDEX turns_edits_uncounted ON turns(started_at)
 WHERE edits_counted_at IS NULL AND status <> 'running';
