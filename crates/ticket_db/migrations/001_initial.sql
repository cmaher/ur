-- ticket_db initial schema.
-- Owns the ticket domain: tickets, activities, metadata, edges, slots, and workers.

CREATE EXTENSION IF NOT EXISTS pgcrypto;

--------------------------------------------------------------------------------
-- ticket
--------------------------------------------------------------------------------
CREATE TABLE ticket (
    id TEXT PRIMARY KEY NOT NULL,
    type TEXT NOT NULL,
    status TEXT NOT NULL,
    priority INTEGER NOT NULL,
    parent_id TEXT,
    title TEXT NOT NULL,
    body TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    project TEXT NOT NULL DEFAULT '',
    lifecycle_status TEXT NOT NULL DEFAULT 'open',
    branch TEXT,
    lifecycle_managed BOOLEAN NOT NULL DEFAULT FALSE,
    FOREIGN KEY (parent_id) REFERENCES ticket(id)
);

CREATE INDEX idx_ticket_parent_id ON ticket(parent_id);
CREATE INDEX idx_ticket_project_priority ON ticket(project, priority);
CREATE INDEX idx_ticket_status ON ticket(status);

--------------------------------------------------------------------------------
-- edge
--------------------------------------------------------------------------------
CREATE TABLE edge (
    source_id TEXT NOT NULL,
    target_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    PRIMARY KEY (source_id, target_id, kind),
    FOREIGN KEY (source_id) REFERENCES ticket(id),
    FOREIGN KEY (target_id) REFERENCES ticket(id)
);

CREATE INDEX idx_edge_target ON edge(target_id, kind);

--------------------------------------------------------------------------------
-- meta
--------------------------------------------------------------------------------
CREATE TABLE meta (
    entity_id TEXT NOT NULL,
    entity_type TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (entity_id, entity_type, key)
);

CREATE INDEX idx_meta_lookup ON meta(entity_type, key, value);

--------------------------------------------------------------------------------
-- activity
--------------------------------------------------------------------------------
CREATE TABLE activity (
    id TEXT PRIMARY KEY NOT NULL,
    ticket_id TEXT NOT NULL,
    "timestamp" TEXT NOT NULL,
    author TEXT NOT NULL,
    message TEXT NOT NULL,
    FOREIGN KEY (ticket_id) REFERENCES ticket(id)
);

CREATE INDEX idx_activity_ticket_id ON activity(ticket_id);

--------------------------------------------------------------------------------
-- slot
--------------------------------------------------------------------------------
CREATE TABLE slot (
    id TEXT PRIMARY KEY NOT NULL,
    project_key TEXT NOT NULL,
    slot_name TEXT NOT NULL,
    host_path TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (now()::TEXT),
    updated_at TEXT NOT NULL DEFAULT (now()::TEXT),
    UNIQUE(project_key, slot_name)
);

CREATE INDEX idx_slot_project ON slot(project_key);

--------------------------------------------------------------------------------
-- worker
--------------------------------------------------------------------------------
CREATE TABLE worker (
    worker_id TEXT PRIMARY KEY NOT NULL,
    process_id TEXT NOT NULL,
    project_key TEXT NOT NULL,
    container_id TEXT NOT NULL,
    worker_secret TEXT NOT NULL,
    strategy TEXT NOT NULL,
    container_status TEXT NOT NULL DEFAULT 'provisioning',
    agent_status TEXT NOT NULL DEFAULT 'starting',
    workspace_path TEXT,
    created_at TEXT NOT NULL DEFAULT (now()::TEXT),
    updated_at TEXT NOT NULL DEFAULT (now()::TEXT),
    idle_redispatch_count INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_worker_container_status ON worker(container_status);
CREATE INDEX idx_worker_process_id ON worker(process_id);

--------------------------------------------------------------------------------
-- worker_slot
--------------------------------------------------------------------------------
CREATE TABLE worker_slot (
    worker_id TEXT NOT NULL,
    slot_id TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (now()::TEXT),
    PRIMARY KEY (worker_id, slot_id),
    FOREIGN KEY (worker_id) REFERENCES worker(worker_id) ON DELETE CASCADE,
    FOREIGN KEY (slot_id) REFERENCES slot(id) ON DELETE CASCADE
);

--------------------------------------------------------------------------------
-- ticket_comments
--------------------------------------------------------------------------------
CREATE TABLE ticket_comments (
    comment_id TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    pr_number BIGINT NOT NULL,
    gh_repo TEXT NOT NULL,
    reply_posted BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TEXT NOT NULL DEFAULT (now()::TEXT),
    PRIMARY KEY (comment_id, ticket_id),
    FOREIGN KEY (ticket_id) REFERENCES ticket(id)
);

CREATE INDEX idx_ticket_comments_pending ON ticket_comments(reply_posted) WHERE reply_posted = FALSE;

--------------------------------------------------------------------------------
-- ui_events
-- NOTE: This DDL is identical to workflow_db/migrations/001_initial.sql.
-- The canonical definition lives in db_events::UI_EVENTS_DDL; both migrations
-- embed it verbatim (copy-paste) because sqlx migrations are file-based and
-- cannot share code across crates at migration time. See crates/db_events/CLAUDE.md.
--------------------------------------------------------------------------------
CREATE TABLE ui_events (
    id BIGSERIAL PRIMARY KEY,
    entity_type TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (now()::TEXT)
);

--------------------------------------------------------------------------------
-- Triggers
--------------------------------------------------------------------------------

-- ticket_lifecycle_change: log lifecycle transitions to workflow_event (local stub)
-- Full workflow_event table lives in workflow_db; this trigger is omitted here.

-- UI event triggers with ancestor propagation (recursive CTE) and pg_notify.

-- Ticket insert
CREATE OR REPLACE FUNCTION ui_events_ticket_insert_fn() RETURNS TRIGGER AS $$
BEGIN
    INSERT INTO ui_events (entity_type, entity_id)
        WITH RECURSIVE ancestors(id) AS (
            SELECT NEW.id
            UNION ALL
            SELECT t.parent_id
            FROM ticket t
            JOIN ancestors a ON t.id = a.id
            WHERE t.parent_id IS NOT NULL
        )
        SELECT 'ticket', id FROM ancestors;
    PERFORM pg_notify('ui_events', '');
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER ui_events_ticket_insert
AFTER INSERT ON ticket
FOR EACH ROW
EXECUTE FUNCTION ui_events_ticket_insert_fn();

-- Ticket update
CREATE OR REPLACE FUNCTION ui_events_ticket_update_fn() RETURNS TRIGGER AS $$
BEGIN
    INSERT INTO ui_events (entity_type, entity_id)
        WITH RECURSIVE ancestors(id) AS (
            SELECT NEW.id
            UNION ALL
            SELECT t.parent_id
            FROM ticket t
            JOIN ancestors a ON t.id = a.id
            WHERE t.parent_id IS NOT NULL
        )
        SELECT 'ticket', id FROM ancestors;
    PERFORM pg_notify('ui_events', '');
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER ui_events_ticket_update
AFTER UPDATE ON ticket
FOR EACH ROW
EXECUTE FUNCTION ui_events_ticket_update_fn();

-- Worker insert
CREATE OR REPLACE FUNCTION ui_events_worker_insert_fn() RETURNS TRIGGER AS $$
BEGIN
    INSERT INTO ui_events (entity_type, entity_id) VALUES ('worker', NEW.worker_id);
    PERFORM pg_notify('ui_events', '');
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER ui_events_worker_insert
AFTER INSERT ON worker
FOR EACH ROW
EXECUTE FUNCTION ui_events_worker_insert_fn();

-- Worker update
CREATE OR REPLACE FUNCTION ui_events_worker_update_fn() RETURNS TRIGGER AS $$
BEGIN
    INSERT INTO ui_events (entity_type, entity_id) VALUES ('worker', NEW.worker_id);
    PERFORM pg_notify('ui_events', '');
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER ui_events_worker_update
AFTER UPDATE ON worker
FOR EACH ROW
EXECUTE FUNCTION ui_events_worker_update_fn();
