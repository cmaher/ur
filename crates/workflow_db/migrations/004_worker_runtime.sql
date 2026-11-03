-- Where the worker's agent runs: a Docker container or natively on the host.
-- DEFAULT backfills existing rows as container; CHECK rejects unknown values.
ALTER TABLE worker ADD COLUMN runtime TEXT NOT NULL DEFAULT 'container'
    CHECK (runtime IN ('container', 'local'));
