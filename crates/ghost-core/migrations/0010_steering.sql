-- User-authored steering is retained independently of transient native control requests.
CREATE TABLE steering_inputs (
    message_id TEXT PRIMARY KEY NOT NULL,
    chat_id TEXT NOT NULL REFERENCES chats(id),
    turn_id TEXT NOT NULL REFERENCES turns(id),
    seq INTEGER NOT NULL,
    parts_json TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('pending','accepted','rejected','unconfirmed')),
    detail TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX steering_inputs_chat_seq ON steering_inputs(chat_id,seq);
CREATE INDEX steering_inputs_turn_seq ON steering_inputs(turn_id,seq);
