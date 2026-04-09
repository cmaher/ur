-- Add node_id column to worker, slot, and workflow tables for multi-node support.

ALTER TABLE worker ADD COLUMN node_id TEXT NOT NULL DEFAULT '';
ALTER TABLE slot ADD COLUMN node_id TEXT NOT NULL DEFAULT '';
ALTER TABLE workflow ADD COLUMN node_id TEXT NOT NULL DEFAULT '';

CREATE INDEX idx_worker_node_id ON worker(node_id);
CREATE INDEX idx_slot_node_id ON slot(node_id);
CREATE INDEX idx_workflow_node_id ON workflow(node_id);
