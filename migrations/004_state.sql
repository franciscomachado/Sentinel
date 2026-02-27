-- Persistent state for watchers and state compiler

CREATE TABLE IF NOT EXISTS watcher_state (
    watcher_id TEXT PRIMARY KEY,
    state_key TEXT NOT NULL,
    state_value TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS memories (
    id TEXT PRIMARY KEY,
    content TEXT NOT NULL,
    tags TEXT NOT NULL DEFAULT '[]',
    source TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT
);

CREATE TABLE IF NOT EXISTS observations (
    id TEXT PRIMARY KEY,
    content TEXT NOT NULL,
    source TEXT NOT NULL DEFAULT 'cortex',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    superseded_by TEXT REFERENCES observations(id)
);

CREATE INDEX idx_memories_tags ON memories(tags);
CREATE INDEX idx_observations_created ON observations(created_at);
