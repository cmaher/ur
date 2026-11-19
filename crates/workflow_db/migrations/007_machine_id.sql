-- Which machine each worker and pool slot lives on (ur-m2qoj). Nullable here
-- because the id is only known at runtime: the server fills every NULL with
-- its own machine id (WorkerRepo::backfill_machine_id) right after running
-- migrations, before anything reads a worker or slot row. Every insert after
-- that writes it.
ALTER TABLE worker ADD COLUMN machine_id TEXT;
ALTER TABLE slot ADD COLUMN machine_id TEXT;

-- Slot names are per machine: two machines may both have slot "3" of a
-- project. NULLS NOT DISTINCT keeps the old per-project guarantee for rows
-- not yet backfilled. The old constraint is 001's
-- UNIQUE(project_key, slot_name), auto-named by Postgres.
ALTER TABLE slot DROP CONSTRAINT slot_project_key_slot_name_key;
ALTER TABLE slot ADD CONSTRAINT slot_machine_project_slot_name_key
    UNIQUE NULLS NOT DISTINCT (machine_id, project_key, slot_name);
-- The constraint's index leads with (machine_id, project_key), so it also
-- serves the machine-scoped slot listings; no separate index is needed.
