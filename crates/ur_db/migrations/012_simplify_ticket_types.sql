-- Convert removed ticket types (epic, bug, feature, chore) to task.
-- "Epic" is now a concept (a ticket with children), not a type.
UPDATE ticket SET type = 'task' WHERE type IN ('epic', 'bug', 'feature', 'chore');
