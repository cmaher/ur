-- Expand workflow table with stall and lifecycle columns.
-- Drop attempts from workflow_intent (stall tracking moves to workflow table).

ALTER TABLE workflow ADD COLUMN stalled INTEGER NOT NULL DEFAULT 0;
ALTER TABLE workflow ADD COLUMN stall_reason TEXT NOT NULL DEFAULT '';
ALTER TABLE workflow ADD COLUMN implement_cycles INTEGER NOT NULL DEFAULT 0;
ALTER TABLE workflow ADD COLUMN worker_id TEXT NOT NULL DEFAULT '';
ALTER TABLE workflow ADD COLUMN noverify INTEGER NOT NULL DEFAULT 0;
ALTER TABLE workflow ADD COLUMN feedback_mode TEXT NOT NULL DEFAULT '';

-- SQLite does not support DROP COLUMN before 3.35.0.
-- Recreate workflow_intent without the attempts column.
CREATE TABLE workflow_intent_new (
    id TEXT PRIMARY KEY NOT NULL,
    ticket_id TEXT NOT NULL,
    target_status TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (ticket_id) REFERENCES ticket(id)
);

INSERT INTO workflow_intent_new (id, ticket_id, target_status, created_at)
    SELECT id, ticket_id, target_status, created_at FROM workflow_intent;

DROP TABLE workflow_intent;
ALTER TABLE workflow_intent_new RENAME TO workflow_intent;

CREATE INDEX idx_workflow_intent_ticket_id ON workflow_intent(ticket_id);
CREATE INDEX idx_workflow_intent_created_at ON workflow_intent(created_at);
