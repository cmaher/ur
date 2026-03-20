CREATE TABLE knowledge (
    id TEXT PRIMARY KEY NOT NULL,
    project TEXT,
    title TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    body TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE knowledge_tag (
    knowledge_id TEXT NOT NULL,
    tag TEXT NOT NULL,
    PRIMARY KEY (knowledge_id, tag),
    FOREIGN KEY (knowledge_id) REFERENCES knowledge(id) ON DELETE CASCADE
);

CREATE INDEX idx_knowledge_project ON knowledge(project);
CREATE INDEX idx_knowledge_tag_tag ON knowledge_tag(tag);
