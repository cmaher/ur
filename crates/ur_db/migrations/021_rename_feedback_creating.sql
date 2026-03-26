-- Rename lifecycle status 'feedback_creating' to 'addressing_feedback' in all workflow tables.
UPDATE workflow SET status = 'addressing_feedback' WHERE status = 'feedback_creating';
UPDATE workflow_events SET event = 'addressing_feedback' WHERE event = 'feedback_creating';
UPDATE workflow_intent SET target_status = 'addressing_feedback' WHERE target_status = 'feedback_creating';
