-- Rhythms: detected patterns from the ledger

CREATE TABLE IF NOT EXISTS rhythms (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    activity TEXT NOT NULL UNIQUE,
    typical_interval_secs INTEGER NOT NULL,
    variance_secs INTEGER NOT NULL DEFAULT 0,
    last_occurrence TEXT NOT NULL,
    occurrences INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'Emerging',
    computed_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_rhythms_status ON rhythms(status);
CREATE INDEX idx_rhythms_activity ON rhythms(activity);
