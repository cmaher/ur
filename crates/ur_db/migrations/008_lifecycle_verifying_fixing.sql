-- Add verifying and fixing lifecycle statuses, remove stalled.
-- Any tickets currently in 'stalled' lifecycle status revert to 'open'.
UPDATE ticket SET lifecycle_status = 'open' WHERE lifecycle_status = 'stalled';

-- Also clean up any workflow events that reference 'stalled'.
DELETE FROM workflow_event WHERE new_lifecycle_status = 'stalled';
