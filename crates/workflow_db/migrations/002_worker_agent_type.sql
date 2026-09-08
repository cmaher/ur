-- Add agent_type to worker, orthogonal to strategy (strategy = what kind of
-- work, agent_type = which AI runs it). DEFAULT backfills existing rows.
ALTER TABLE worker ADD COLUMN agent_type TEXT NOT NULL DEFAULT 'claude';
