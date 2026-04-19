-- workflow_db initial schema.
-- Owns the workflow domain: worker/slot lifecycle, workflow state, events, intents, and comments.
-- No node_id columns anywhere. ticket_id columns are plain TEXT with no FK constraints.

CREATE EXTENSION IF NOT EXISTS pgcrypto;

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
-- workflow
--------------------------------------------------------------------------------
CREATE TABLE workflow (
    id TEXT PRIMARY KEY NOT NULL,
    ticket_id TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'open',
    created_at TEXT NOT NULL DEFAULT (now()::TEXT),
    stalled BOOLEAN NOT NULL DEFAULT FALSE,
    stall_reason TEXT NOT NULL DEFAULT '',
    implement_cycles INTEGER NOT NULL DEFAULT 0,
    worker_id TEXT NOT NULL DEFAULT '',
    noverify BOOLEAN NOT NULL DEFAULT FALSE,
    feedback_mode TEXT NOT NULL DEFAULT '',
    ci_status TEXT NOT NULL DEFAULT 'pending',
    mergeable TEXT NOT NULL DEFAULT 'unknown',
    review_status TEXT NOT NULL DEFAULT 'pending'
);

CREATE INDEX idx_workflow_ticket_id ON workflow(ticket_id);
CREATE INDEX idx_workflow_status ON workflow(status);

--------------------------------------------------------------------------------
-- workflow_event
--------------------------------------------------------------------------------
CREATE TABLE workflow_event (
    id TEXT PRIMARY KEY NOT NULL,
    ticket_id TEXT NOT NULL,
    old_lifecycle_status TEXT NOT NULL,
    new_lifecycle_status TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (now()::TEXT)
);

CREATE INDEX idx_workflow_event_ticket_id ON workflow_event(ticket_id);
CREATE INDEX idx_workflow_event_created_at ON workflow_event(created_at);

--------------------------------------------------------------------------------
-- workflow_intent
--------------------------------------------------------------------------------
CREATE TABLE workflow_intent (
    id TEXT PRIMARY KEY NOT NULL,
    ticket_id TEXT NOT NULL,
    target_status TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (now()::TEXT)
);

CREATE INDEX idx_workflow_intent_ticket_id ON workflow_intent(ticket_id);
CREATE INDEX idx_workflow_intent_created_at ON workflow_intent(created_at);

--------------------------------------------------------------------------------
-- workflow_comments
--------------------------------------------------------------------------------
CREATE TABLE workflow_comments (
    ticket_id TEXT NOT NULL,
    comment_id TEXT NOT NULL,
    feedback_created BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TEXT NOT NULL DEFAULT (now()::TEXT),
    PRIMARY KEY (ticket_id, comment_id)
);

CREATE INDEX idx_workflow_comments_ticket_id ON workflow_comments(ticket_id);

--------------------------------------------------------------------------------
-- workflow_events (event log)
--------------------------------------------------------------------------------
CREATE TABLE workflow_events (
    id TEXT PRIMARY KEY NOT NULL,
    workflow_id TEXT NOT NULL,
    event TEXT NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY (workflow_id) REFERENCES workflow(id)
);

CREATE INDEX idx_workflow_events_workflow_created ON workflow_events(workflow_id, created_at);

--------------------------------------------------------------------------------
-- ui_events
-- NOTE: This DDL is identical to ticket_db/migrations/001_initial.sql.
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

-- Workflow insert
CREATE OR REPLACE FUNCTION ui_events_workflow_insert_fn() RETURNS TRIGGER AS $$
BEGIN
    INSERT INTO ui_events (entity_type, entity_id) VALUES ('workflow', NEW.ticket_id);
    PERFORM pg_notify('ui_events', '');
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER ui_events_workflow_insert
AFTER INSERT ON workflow
FOR EACH ROW
EXECUTE FUNCTION ui_events_workflow_insert_fn();

-- Workflow update
CREATE OR REPLACE FUNCTION ui_events_workflow_update_fn() RETURNS TRIGGER AS $$
BEGIN
    INSERT INTO ui_events (entity_type, entity_id) VALUES ('workflow', NEW.ticket_id);
    PERFORM pg_notify('ui_events', '');
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER ui_events_workflow_update
AFTER UPDATE ON workflow
FOR EACH ROW
EXECUTE FUNCTION ui_events_workflow_update_fn();

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
