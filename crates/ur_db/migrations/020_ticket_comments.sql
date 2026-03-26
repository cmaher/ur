-- Track comment→ticket relationships for auto-reply posting.
CREATE TABLE ticket_comments (
    comment_id TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    pr_number INTEGER NOT NULL,
    gh_repo TEXT NOT NULL,
    reply_posted INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (comment_id, ticket_id),
    FOREIGN KEY (ticket_id) REFERENCES ticket(id)
);

CREATE INDEX idx_ticket_comments_pending ON ticket_comments(reply_posted) WHERE reply_posted = 0;
