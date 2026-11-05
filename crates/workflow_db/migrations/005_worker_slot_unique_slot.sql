-- One owner per slot (ur-j5puq): the worker_slot row is the slot claim, so
-- inserting it is what decides who gets a slot.
--
-- Existing links are resolved first, by this rule:
--   1. A link whose worker is not active (stopped; a missing worker cannot
--      exist because of the FK) owns nothing and is removed.
--   2. A slot still linked to more than one active worker keeps the link of
--      a `running` worker over a `provisioning`/`stopping` one, then the
--      newest (latest created_at, then highest worker_id). Each losing worker
--      is moved to `stopping` (it must not stay active without a slot) and
--      its link removed; the stop paths then converge it.
DELETE FROM worker_slot ws
WHERE NOT EXISTS (
    SELECT 1 FROM worker w
    WHERE w.worker_id = ws.worker_id
      AND w.container_status IN ('provisioning', 'running', 'stopping')
);

WITH losers AS (
    SELECT worker_id, slot_id FROM (
        SELECT l.worker_id, l.slot_id,
               ROW_NUMBER() OVER (
                   PARTITION BY l.slot_id
                   ORDER BY (w.container_status = 'running') DESC,
                            w.created_at DESC, w.worker_id DESC
               ) AS rank
        FROM worker_slot l
        JOIN worker w ON w.worker_id = l.worker_id
    ) ranked
    WHERE rank > 1
), moved AS (
    UPDATE worker SET container_status = 'stopping'
    WHERE worker_id IN (SELECT worker_id FROM losers)
)
DELETE FROM worker_slot ws
USING losers lost
WHERE ws.worker_id = lost.worker_id AND ws.slot_id = lost.slot_id;

ALTER TABLE worker_slot ADD CONSTRAINT worker_slot_one_owner UNIQUE (slot_id);
