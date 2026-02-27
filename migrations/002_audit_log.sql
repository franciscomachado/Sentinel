-- Audit log

CREATE TABLE IF NOT EXISTS audit_log (
    id TEXT PRIMARY KEY,
    timestamp TEXT NOT NULL DEFAULT (datetime('now')),
    capability_type TEXT NOT NULL,
    capability_data TEXT NOT NULL,
    source TEXT NOT NULL,
    decision TEXT NOT NULL,
    cortex_reasoning TEXT NOT NULL DEFAULT '',
    execution_result TEXT,
    input_tokens INTEGER,
    output_tokens INTEGER,
    cached_tokens INTEGER
);

CREATE INDEX idx_audit_timestamp ON audit_log(timestamp);
CREATE INDEX idx_audit_decision ON audit_log(decision);
