CREATE TABLE slot (
    id TEXT PRIMARY KEY NOT NULL,
    project_key TEXT NOT NULL,
    slot_name TEXT NOT NULL,
    slot_type TEXT NOT NULL,
    host_path TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'available',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(project_key, slot_name)
);

CREATE TABLE worker (
    worker_id TEXT PRIMARY KEY NOT NULL,
    process_id TEXT NOT NULL,
    project_key TEXT NOT NULL,
    slot_id TEXT,
    container_id TEXT NOT NULL,
    worker_secret TEXT NOT NULL,
    strategy TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'provisioning',
    workspace_path TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (slot_id) REFERENCES slot(id)
);

CREATE INDEX idx_worker_status ON worker(status);
CREATE INDEX idx_worker_process_id ON worker(process_id);
CREATE INDEX idx_slot_project ON slot(project_key, status);
