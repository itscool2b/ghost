ALTER TABLE queued_prompts ADD COLUMN parts_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE turns ADD COLUMN prompt_parts_json TEXT NOT NULL DEFAULT '[]';

CREATE TABLE composer_attachments (
    id TEXT PRIMARY KEY,
    chat_id TEXT NOT NULL REFERENCES chats(id),
    sha256 TEXT NOT NULL,
    display_name TEXT NOT NULL,
    mime TEXT NOT NULL,
    byte_length INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE(chat_id, sha256, display_name)
);
CREATE INDEX composer_attachments_chat ON composer_attachments(chat_id);
