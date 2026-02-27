-- Ledger: append-only event log

CREATE TABLE IF NOT EXISTS ledger (
    id TEXT PRIMARY KEY,
    timestamp TEXT NOT NULL DEFAULT (datetime('now')),
    category TEXT NOT NULL,
    content TEXT NOT NULL,
    tags TEXT NOT NULL DEFAULT '[]',
    source TEXT NOT NULL
);

CREATE INDEX idx_ledger_timestamp ON ledger(timestamp);
CREATE INDEX idx_ledger_category ON ledger(category);
CREATE INDEX idx_ledger_tags ON ledger(tags);
