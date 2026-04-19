-- workflow_db initial schema.
-- Owns the workflow domain: workflow state, events, intents, and comments.

CREATE EXTENSION IF NOT EXISTS pgcrypto;

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
