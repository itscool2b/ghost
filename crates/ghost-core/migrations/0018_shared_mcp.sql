CREATE TABLE shared_mcp (
    scope TEXT NOT NULL,
    server_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    definition_json TEXT NOT NULL,
    local_json TEXT NOT NULL DEFAULT '{}',
    PRIMARY KEY(scope, server_id)
);
CREATE TABLE shared_mcp_settings (
    id INTEGER PRIMARY KEY CHECK(id = 1),
    revision INTEGER NOT NULL DEFAULT 0
);
INSERT INTO shared_mcp_settings(id, revision) VALUES(1, 0);
CREATE TABLE shared_mcp_heads(scope TEXT NOT NULL, server_id TEXT NOT NULL, head TEXT NOT NULL, PRIMARY KEY(scope,server_id));
CREATE TABLE shared_mcp_revisions(id TEXT PRIMARY KEY, parent TEXT, scope TEXT NOT NULL, server_id TEXT NOT NULL, definition_json TEXT, conflict INTEGER NOT NULL DEFAULT 0, resolved_json TEXT NOT NULL DEFAULT '[]');
