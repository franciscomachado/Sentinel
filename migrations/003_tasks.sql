-- Tasks

CREATE TABLE IF NOT EXISTS tasks (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    notes TEXT,
    schedule_type TEXT NOT NULL,
    schedule_data TEXT NOT NULL,
    next_trigger TEXT,
    context TEXT NOT NULL DEFAULT '[]',
    conditions TEXT NOT NULL DEFAULT '[]',
    urgency TEXT NOT NULL DEFAULT 'Medium',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    created_by TEXT NOT NULL,
    completed_at TEXT
);

CREATE TABLE IF NOT EXISTS task_completions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL REFERENCES tasks(id),
    completed_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_tasks_next_trigger ON tasks(next_trigger);
CREATE INDEX idx_task_completions_task ON task_completions(task_id);
