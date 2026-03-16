-- Create worker_slot join table
CREATE TABLE worker_slot (
    worker_id TEXT NOT NULL,
    slot_id TEXT NOT NULL,
    PRIMARY KEY (worker_id, slot_id),
    FOREIGN KEY (worker_id) REFERENCES worker(worker_id) ON DELETE CASCADE,
    FOREIGN KEY (slot_id) REFERENCES slot(id) ON DELETE CASCADE
);

-- Recreate slot table without status column
CREATE TABLE slot_new (
    id TEXT PRIMARY KEY NOT NULL,
    project_key TEXT NOT NULL,
    slot_name TEXT NOT NULL,
    slot_type TEXT NOT NULL,
    host_path TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(project_key, slot_name)
);

INSERT INTO slot_new (id, project_key, slot_name, slot_type, host_path, created_at, updated_at)
    SELECT id, project_key, slot_name, slot_type, host_path, created_at, updated_at FROM slot;

DROP TABLE slot;
ALTER TABLE slot_new RENAME TO slot;

-- Recreate idx_slot_project as (project_key) only
CREATE INDEX idx_slot_project ON slot(project_key);

-- Migrate existing worker.slot_id data into worker_slot
INSERT INTO worker_slot (worker_id, slot_id)
    SELECT worker_id, slot_id FROM worker WHERE slot_id IS NOT NULL;

-- Recreate worker table without slot_id column
CREATE TABLE worker_new (
    worker_id TEXT PRIMARY KEY NOT NULL,
    process_id TEXT NOT NULL,
    project_key TEXT NOT NULL,
    container_id TEXT NOT NULL,
    worker_secret TEXT NOT NULL,
    strategy TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'provisioning',
    workspace_path TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

INSERT INTO worker_new (worker_id, process_id, project_key, container_id, worker_secret, strategy, status, workspace_path, created_at, updated_at)
    SELECT worker_id, process_id, project_key, container_id, worker_secret, strategy, status, workspace_path, created_at, updated_at FROM worker;

DROP TABLE worker;
ALTER TABLE worker_new RENAME TO worker;

-- Recreate worker indexes
CREATE INDEX idx_worker_status ON worker(status);
CREATE INDEX idx_worker_process_id ON worker(process_id);
