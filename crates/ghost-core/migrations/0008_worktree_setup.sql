-- Device-local recipes stay in the profile database, including SQLCipher Account profiles.
-- No file contents, output, credentials or execution grants are persisted here.
CREATE TABLE worktree_setup (
    folder_id TEXT NOT NULL REFERENCES project_folders(id),
    device_id TEXT NOT NULL,
    input_json TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(folder_id, device_id)
);
