-- Empty in Local profiles. Account sync initializes its identity only after a keyed open.
CREATE TABLE sync_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    account_id TEXT NOT NULL,
    device_id TEXT NOT NULL,
    key_version INTEGER NOT NULL,
    key_fingerprint TEXT NOT NULL,
    last_hlc TEXT NOT NULL,
    pull_cursor INTEGER NOT NULL DEFAULT 0 CHECK (pull_cursor >= 0)
);
CREATE TABLE sync_heads (
    collection TEXT NOT NULL,
    id TEXT NOT NULL,
    record_json TEXT NOT NULL,
    PRIMARY KEY (collection, id)
);
CREATE TABLE sync_outbox (
    collection TEXT NOT NULL,
    id TEXT NOT NULL,
    hlc TEXT NOT NULL,
    record_json TEXT NOT NULL,
    PRIMARY KEY (collection, id)
);
CREATE INDEX sync_outbox_order ON sync_outbox (hlc, collection, id);
-- Keep each unsupported authenticated revision before advancing the cursor. A newer client can
-- process these later; an older client may neither overwrite them nor push that item's local head.
CREATE TABLE sync_deferred (
    collection TEXT NOT NULL,
    id TEXT NOT NULL,
    hlc TEXT NOT NULL,
    server_seq INTEGER NOT NULL,
    reason TEXT NOT NULL,
    record_json TEXT NOT NULL,
    PRIMARY KEY (collection, id, hlc)
);
