-- The agent, model and thinking level a chat plans and builds with (owner, 19 November 2026:
-- models by role are "a chat by chat thing not a settings thing"). A row exists only for a role
-- the person chose; none means the chat changes nothing. It stays on this device.
CREATE TABLE chat_roles (
 chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
 role TEXT NOT NULL CHECK(role IN ('plan','build')),
 agent TEXT NOT NULL,
 model TEXT,
 effort TEXT,
 updated_at INTEGER NOT NULL,
 PRIMARY KEY(chat_id,role)
);
