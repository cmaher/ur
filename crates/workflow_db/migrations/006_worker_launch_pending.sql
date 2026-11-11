-- A worker whose own Launch RPC is unresolved (ur-ezhdb). While it is set,
-- only the launch path (or the server's startup pass) may finish a stop of
-- the worker: another stopper's builderd Stop may have reached a builderd
-- that then restarted, so its Ok does not cover a Launch not yet delivered.
ALTER TABLE worker ADD COLUMN launch_pending BOOLEAN NOT NULL DEFAULT false;
