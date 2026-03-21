-- Add condition columns to workflow for parallel InReview tracking.
ALTER TABLE workflow ADD COLUMN ci_status TEXT NOT NULL DEFAULT 'pending';
ALTER TABLE workflow ADD COLUMN mergeable TEXT NOT NULL DEFAULT 'unknown';
ALTER TABLE workflow ADD COLUMN review_status TEXT NOT NULL DEFAULT 'pending';

-- New table for workflow event log entries.
CREATE TABLE workflow_events (
    id TEXT PRIMARY KEY NOT NULL,
    workflow_id TEXT NOT NULL,
    event TEXT NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY (workflow_id) REFERENCES workflow(id)
);

CREATE INDEX idx_workflow_events_workflow_created ON workflow_events(workflow_id, created_at);
