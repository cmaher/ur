-- Add workflow and workflow_intent tables for the new intent-driven workflow engine.

CREATE TABLE workflow (
    id TEXT PRIMARY KEY NOT NULL,
    ticket_id TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL DEFAULT 'open',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (ticket_id) REFERENCES ticket(id)
);

CREATE INDEX idx_workflow_ticket_id ON workflow(ticket_id);
CREATE INDEX idx_workflow_status ON workflow(status);

CREATE TABLE workflow_intent (
    id TEXT PRIMARY KEY NOT NULL,
    ticket_id TEXT NOT NULL,
    target_status TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (ticket_id) REFERENCES ticket(id)
);

CREATE INDEX idx_workflow_intent_ticket_id ON workflow_intent(ticket_id);
CREATE INDEX idx_workflow_intent_created_at ON workflow_intent(created_at);
