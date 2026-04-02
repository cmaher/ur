-- Rename ticket type "task" to "code" (canonical form).
UPDATE ticket SET type = 'code' WHERE type = 'task';
