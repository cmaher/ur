-- Index on ticket status for efficient filtering by status in TUI and queries.
CREATE INDEX IF NOT EXISTS idx_ticket_status ON ticket(status);
