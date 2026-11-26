-- Authenticated remote metadata waits here until its parent rows have arrived. The cursor and
-- queue commit together; a missing dependency never means deletion or a fabricated local path.
CREATE TABLE sync_projection (
    collection TEXT NOT NULL,
    id TEXT NOT NULL,
    attempt INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (collection, id),
    FOREIGN KEY (collection, id) REFERENCES sync_heads (collection, id)
);
CREATE INDEX sync_projection_attempt ON sync_projection (attempt, collection, id);
CREATE INDEX sync_projection_id ON sync_projection (id);
