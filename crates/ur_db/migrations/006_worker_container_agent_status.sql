-- Rename worker.status to container_status and add agent_status column.
-- SQLite does not support ALTER TABLE RENAME COLUMN before 3.25, so we
-- recreate the table.

CREATE TABLE worker_new (
    worker_id TEXT PRIMARY KEY NOT NULL,
    process_id TEXT NOT NULL,
    project_key TEXT NOT NULL,
    container_id TEXT NOT NULL,
    worker_secret TEXT NOT NULL,
    strategy TEXT NOT NULL,
    container_status TEXT NOT NULL DEFAULT 'provisioning',
    agent_status TEXT NOT NULL DEFAULT 'starting',
    workspace_path TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

INSERT INTO worker_new (worker_id, process_id, project_key, container_id, worker_secret, strategy, container_status, agent_status, workspace_path, created_at, updated_at)
    SELECT worker_id, process_id, project_key, container_id, worker_secret, strategy, status, 'starting', workspace_path, created_at, updated_at FROM worker;

DROP TABLE worker;
ALTER TABLE worker_new RENAME TO worker;

-- Recreate worker indexes (now on container_status instead of status).
CREATE INDEX idx_worker_container_status ON worker(container_status);
CREATE INDEX idx_worker_process_id ON worker(process_id);
