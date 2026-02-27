-- Email metadata cache

CREATE TABLE IF NOT EXISTS email_cache (
    id TEXT PRIMARY KEY,
    account TEXT NOT NULL,
    uid INTEGER NOT NULL,
    from_addr TEXT NOT NULL,
    to_addrs TEXT NOT NULL,
    subject TEXT NOT NULL,
    preview TEXT,
    timestamp TEXT NOT NULL,
    is_reply INTEGER NOT NULL DEFAULT 0,
    has_attachments INTEGER NOT NULL DEFAULT 0,
    urgency TEXT NOT NULL DEFAULT 'Medium',
    triaged INTEGER NOT NULL DEFAULT 0,
    cached_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_email_account ON email_cache(account);
CREATE INDEX idx_email_from ON email_cache(from_addr);
CREATE INDEX idx_email_timestamp ON email_cache(timestamp);
