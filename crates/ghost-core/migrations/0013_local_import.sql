-- Device-local receipts prevent an explicit Local import from overwriting an Account project.
-- The enclosing Account database protects the receipt; it is never projected to the server.
CREATE TABLE local_imports (
    project_id TEXT PRIMARY KEY REFERENCES projects(id),
    source_profile TEXT NOT NULL,
    source_revision TEXT NOT NULL,
    imported_at INTEGER NOT NULL
);
