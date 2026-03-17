-- Remove slot_type column from slot table (all slots are now exclusive).
CREATE TABLE slot_new (
    id TEXT PRIMARY KEY NOT NULL,
    project_key TEXT NOT NULL,
    slot_name TEXT NOT NULL,
    host_path TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(project_key, slot_name)
);

INSERT INTO slot_new (id, project_key, slot_name, host_path, created_at, updated_at)
    SELECT id, project_key, slot_name, host_path, created_at, updated_at FROM slot;

DROP TABLE slot;
ALTER TABLE slot_new RENAME TO slot;

-- Recreate index.
CREATE INDEX idx_slot_project ON slot(project_key);
