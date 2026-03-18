-- Add idle_redispatch_count to track how many times a worker has been
-- nudged with a re-dispatch after reporting idle.
ALTER TABLE worker ADD COLUMN idle_redispatch_count INTEGER NOT NULL DEFAULT 0;
