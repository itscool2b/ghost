-- Device-local execution state. Existing chats have no pending setup.
CREATE TABLE worktree_setup_attempts (
    chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    device_id TEXT NOT NULL,
    worktree_id TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK(generation > 0),
    state TEXT NOT NULL CHECK(state IN ('awaiting','running','failed','interrupted','succeeded')),
    detail TEXT,
    PRIMARY KEY(chat_id, device_id)
);
