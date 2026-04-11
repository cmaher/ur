-- Multi-node fix: scope slot uniqueness by node_id so two nodes can each have
-- their own slot named e.g. "0" for the same project_key.
--
-- Migration 002 added node_id but did not update this constraint, which made
-- slot reconciliation fail on every node-2+ startup.

ALTER TABLE slot DROP CONSTRAINT IF EXISTS slot_project_key_slot_name_key;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'slot_project_key_slot_name_node_id_key'
    ) THEN
        ALTER TABLE slot ADD CONSTRAINT slot_project_key_slot_name_node_id_key
            UNIQUE (project_key, slot_name, node_id);
    END IF;
END$$;
