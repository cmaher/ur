-- Ephemeral UI event buffer. Triggers populate rows on data changes;
-- the UiEventPoller consumes and deletes them.

CREATE TABLE ui_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_type TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Ticket triggers
CREATE TRIGGER ui_events_ticket_insert
AFTER INSERT ON ticket
BEGIN
    INSERT INTO ui_events (entity_type, entity_id) VALUES ('ticket', NEW.id);
END;

CREATE TRIGGER ui_events_ticket_update
AFTER UPDATE ON ticket
BEGIN
    INSERT INTO ui_events (entity_type, entity_id) VALUES ('ticket', NEW.id);
END;

-- Workflow triggers
CREATE TRIGGER ui_events_workflow_insert
AFTER INSERT ON workflow
BEGIN
    INSERT INTO ui_events (entity_type, entity_id) VALUES ('workflow', NEW.ticket_id);
END;

CREATE TRIGGER ui_events_workflow_update
AFTER UPDATE ON workflow
BEGIN
    INSERT INTO ui_events (entity_type, entity_id) VALUES ('workflow', NEW.ticket_id);
END;

-- Worker triggers
CREATE TRIGGER ui_events_worker_insert
AFTER INSERT ON worker
BEGIN
    INSERT INTO ui_events (entity_type, entity_id) VALUES ('worker', NEW.worker_id);
END;

CREATE TRIGGER ui_events_worker_update
AFTER UPDATE ON worker
BEGIN
    INSERT INTO ui_events (entity_type, entity_id) VALUES ('worker', NEW.worker_id);
END;
