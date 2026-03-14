CREATE TABLE ticket (
    id TEXT PRIMARY KEY NOT NULL,
    type TEXT NOT NULL,
    status TEXT NOT NULL,
    priority INTEGER NOT NULL,
    parent_id TEXT,
    title TEXT NOT NULL,
    body TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (parent_id) REFERENCES ticket(id)
);

CREATE TABLE edge (
    source_id TEXT NOT NULL,
    target_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    PRIMARY KEY (source_id, target_id, kind),
    FOREIGN KEY (source_id) REFERENCES ticket(id),
    FOREIGN KEY (target_id) REFERENCES ticket(id)
);

CREATE TABLE meta (
    entity_id TEXT NOT NULL,
    entity_type TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (entity_id, entity_type, key)
);

CREATE TABLE activity (
    id TEXT PRIMARY KEY NOT NULL,
    ticket_id TEXT NOT NULL,
    timestamp TEXT NOT NULL,
    author TEXT NOT NULL,
    message TEXT NOT NULL,
    FOREIGN KEY (ticket_id) REFERENCES ticket(id)
);

CREATE INDEX idx_edge_target ON edge(target_id, kind);
CREATE INDEX idx_activity_ticket_id ON activity(ticket_id);
CREATE INDEX idx_meta_lookup ON meta(entity_type, key, value);
