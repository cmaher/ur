-- One owner per slot (ur-j5puq): the worker_slot row is the slot claim, so
-- inserting it is what decides who gets a slot.
--
-- Existing links are resolved first, by this rule:
--   1. A link whose worker is not active (stopped; a missing worker cannot
--      exist because of the FK) owns nothing and is removed.
--   2. A slot still linked to more than one active worker keeps only the
--      link of the newest worker (latest created_at, then highest worker_id);
--      the others are removed. Their rows are left as they are, and the
--      local reconcile loop stops any that builderd no longer runs.
DELETE FROM worker_slot ws
WHERE NOT EXISTS (
    SELECT 1 FROM worker w
    WHERE w.worker_id = ws.worker_id
      AND w.container_status IN ('provisioning', 'running', 'stopping')
);

DELETE FROM worker_slot ws
USING (
    SELECT l.worker_id, l.slot_id,
           ROW_NUMBER() OVER (
               PARTITION BY l.slot_id ORDER BY w.created_at DESC, w.worker_id DESC
           ) AS rank
    FROM worker_slot l
    JOIN worker w ON w.worker_id = l.worker_id
) ranked
WHERE ws.worker_id = ranked.worker_id
  AND ws.slot_id = ranked.slot_id
  AND ranked.rank > 1;

ALTER TABLE worker_slot ADD CONSTRAINT worker_slot_one_owner UNIQUE (slot_id);
