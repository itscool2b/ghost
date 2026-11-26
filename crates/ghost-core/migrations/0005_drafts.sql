-- Unsent messages belong to the local profile and survive view changes and restarts.
CREATE TABLE composer_drafts (
    chat_id TEXT PRIMARY KEY REFERENCES chats(id),
    input_json TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
