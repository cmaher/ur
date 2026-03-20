-- Track which PR comments have been seen by the GitHub poller
-- and which have had feedback tickets created.
CREATE TABLE workflow_comments (
    ticket_id TEXT NOT NULL,
    comment_id TEXT NOT NULL,
    feedback_created INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (ticket_id, comment_id),
    FOREIGN KEY (ticket_id) REFERENCES ticket(id)
);

CREATE INDEX idx_workflow_comments_ticket_id ON workflow_comments(ticket_id);
