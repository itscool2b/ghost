-- Account-only projections and transfer queue. Local profiles leave these empty. The primary
-- SQLCipher connection protects logical item IDs, merge bases and device-only decisions at rest.
CREATE TABLE sync_config_device (
    id TEXT PRIMARY KEY,
    revision TEXT NOT NULL,
    head_json TEXT,
    snapshot BLOB,
    device_only INTEGER NOT NULL,
    skipped TEXT
);
-- Ciphertext may be queued offline. A head must not leave the device before its referenced blob
-- upload is acknowledged. Old referenced blobs remain available for merge bases and recovery.
CREATE TABLE sync_blobs (
    id TEXT PRIMARY KEY,
    key_version INTEGER NOT NULL,
    nonce TEXT NOT NULL,
    ciphertext TEXT NOT NULL,
    etag TEXT,
    uploaded INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE sync_blob_refs (
    collection TEXT NOT NULL,
    id TEXT NOT NULL,
    blob_id TEXT NOT NULL,
    PRIMARY KEY(collection,id)
);
CREATE INDEX sync_blob_refs_blob ON sync_blob_refs(blob_id);
