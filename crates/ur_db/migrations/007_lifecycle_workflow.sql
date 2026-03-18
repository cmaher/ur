-- Add lifecycle_status and branch columns to ticket.
ALTER TABLE ticket ADD COLUMN lifecycle_status TEXT NOT NULL DEFAULT 'open';
ALTER TABLE ticket ADD COLUMN branch TEXT;

-- Workflow event table to track lifecycle transitions.
CREATE TABLE workflow_event (
    id TEXT PRIMARY KEY NOT NULL,
    ticket_id TEXT NOT NULL,
    old_lifecycle_status TEXT NOT NULL,
    new_lifecycle_status TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (ticket_id) REFERENCES ticket(id)
);

CREATE INDEX idx_workflow_event_ticket_id ON workflow_event(ticket_id);
CREATE INDEX idx_workflow_event_created_at ON workflow_event(created_at);

-- Add idle_redispatch_count to track how many times a worker has been
-- nudged with a re-dispatch after reporting idle.
ALTER TABLE worker ADD COLUMN idle_redispatch_count INTEGER NOT NULL DEFAULT 0;

-- Trigger: automatically log lifecycle transitions.
CREATE TRIGGER ticket_lifecycle_change
AFTER UPDATE OF lifecycle_status ON ticket
WHEN OLD.lifecycle_status <> NEW.lifecycle_status
BEGIN
    INSERT INTO workflow_event (id, ticket_id, old_lifecycle_status, new_lifecycle_status, attempts, created_at)
    VALUES (
        lower(hex(randomblob(8))),
        NEW.id,
        OLD.lifecycle_status,
        NEW.lifecycle_status,
        0,
        datetime('now')
    );
END;
