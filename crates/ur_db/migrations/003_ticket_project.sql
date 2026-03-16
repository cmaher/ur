-- Add project column to ticket table.
-- Backfill from the ticket ID prefix (everything before the first '-').
ALTER TABLE ticket ADD COLUMN project TEXT NOT NULL DEFAULT '';

UPDATE ticket SET project = substr(id, 1, instr(id, '-') - 1) WHERE project = '' AND instr(id, '-') > 0;
UPDATE ticket SET project = id WHERE project = '';
