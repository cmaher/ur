-- Drop the UNIQUE constraint on workflow.ticket_id so that terminal
-- workflows (done/cancelled) are preserved when a new workflow is created.

CREATE TABLE workflow_new (
    id TEXT PRIMARY KEY NOT NULL,
    ticket_id TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'open',
    created_at TEXT NOT NULL,
    stalled INTEGER NOT NULL DEFAULT 0,
    stall_reason TEXT NOT NULL DEFAULT '',
    implement_cycles INTEGER NOT NULL DEFAULT 0,
    worker_id TEXT NOT NULL DEFAULT '',
    noverify INTEGER NOT NULL DEFAULT 0,
    feedback_mode TEXT NOT NULL DEFAULT '',
    FOREIGN KEY (ticket_id) REFERENCES ticket(id)
);

INSERT INTO workflow_new (id, ticket_id, status, created_at, stalled, stall_reason, implement_cycles, worker_id, noverify, feedback_mode)
    SELECT id, ticket_id, status, created_at, stalled, stall_reason, implement_cycles, worker_id, noverify, feedback_mode FROM workflow;

DROP TABLE workflow;
ALTER TABLE workflow_new RENAME TO workflow;

CREATE INDEX idx_workflow_ticket_id ON workflow(ticket_id);
CREATE INDEX idx_workflow_status ON workflow(status);
