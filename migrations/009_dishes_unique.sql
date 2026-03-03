-- Deduplicate dishes keeping the earliest entry per name, then enforce uniqueness.
-- SQLite does not support ADD CONSTRAINT on existing tables, so we recreate via
-- a new table and an index.

DELETE FROM dishes
WHERE id NOT IN (
    SELECT MIN(id)
    FROM dishes
    GROUP BY lower(name)
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_dishes_name ON dishes (lower(name));
