-- Household shared surface

CREATE TABLE IF NOT EXISTS family_events (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    start_time TEXT NOT NULL,
    end_time TEXT,
    location TEXT,
    notes TEXT,
    created_by TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS dishes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    protein TEXT,
    carb TEXT,
    notes TEXT,
    last_cooked TEXT,
    frequency INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS meal_plan (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    date TEXT NOT NULL,
    meal_type TEXT NOT NULL,
    description TEXT NOT NULL,
    ingredients TEXT NOT NULL DEFAULT '[]',
    created_by TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS shopping_list (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    item TEXT NOT NULL,
    category TEXT,
    added_by TEXT NOT NULL,
    context TEXT,
    added_at TEXT NOT NULL DEFAULT (datetime('now')),
    purchased INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS household_tasks (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    assigned_to TEXT,
    schedule_type TEXT NOT NULL,
    schedule_data TEXT NOT NULL,
    next_trigger TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_meal_plan_date ON meal_plan(date);
CREATE INDEX idx_shopping_purchased ON shopping_list(purchased);
CREATE INDEX idx_family_events_start ON family_events(start_time);
