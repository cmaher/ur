-- Replace ticket UI event triggers with versions that propagate events
-- to all ancestors via recursive CTE on parent_id.

DROP TRIGGER IF EXISTS ui_events_ticket_insert;
DROP TRIGGER IF EXISTS ui_events_ticket_update;

CREATE TRIGGER ui_events_ticket_insert
AFTER INSERT ON ticket
BEGIN
    INSERT INTO ui_events (entity_type, entity_id)
        WITH RECURSIVE ancestors(id) AS (
            SELECT NEW.id
            UNION ALL
            SELECT t.parent_id
            FROM ticket t
            JOIN ancestors a ON t.id = a.id
            WHERE t.parent_id IS NOT NULL
        )
        SELECT 'ticket', id FROM ancestors;
END;

CREATE TRIGGER ui_events_ticket_update
AFTER UPDATE ON ticket
BEGIN
    INSERT INTO ui_events (entity_type, entity_id)
        WITH RECURSIVE ancestors(id) AS (
            SELECT NEW.id
            UNION ALL
            SELECT t.parent_id
            FROM ticket t
            JOIN ancestors a ON t.id = a.id
            WHERE t.parent_id IS NOT NULL
        )
        SELECT 'ticket', id FROM ancestors;
END;
