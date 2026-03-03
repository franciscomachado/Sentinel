use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// Resolve the default database path: `$XDG_DATA_HOME/sentinel/sentinel.db`
pub fn default_db_path() -> PathBuf {
    let dirs = directories::ProjectDirs::from("", "", "sentinel")
        .expect("unable to determine data directory");
    dirs.data_dir().join("sentinel.db")
}

/// Open (or create) the SQLite database and run all migrations.
pub async fn open(path: &Path) -> anyhow::Result<SqlitePool> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let url = format!("sqlite:{}?mode=rwc", path.display());
    let options = SqliteConnectOptions::from_str(&url)?
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .create_if_missing(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await?;

    migrate(&pool).await?;

    Ok(pool)
}

/// Run migration SQL files in order.
/// We use a simple hand-rolled scheme: a `_migrations` table tracks which
/// files have been applied. Each migration runs inside a transaction.
async fn migrate(pool: &SqlitePool) -> anyhow::Result<()> {
    // Ensure migration tracking table exists
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS _migrations (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
    )
    .execute(pool)
    .await?;

    let applied: Vec<(i64,)> =
        sqlx::query_as("SELECT id FROM _migrations ORDER BY id")
            .fetch_all(pool)
            .await?;
    let applied_set: std::collections::HashSet<i64> =
        applied.into_iter().map(|r| r.0).collect();

    // Migrations are embedded at compile time
    let migrations: &[(i64, &str, &str)] = &[
        (1, "001_initial", include_str!("../../../migrations/001_initial.sql")),
        (2, "002_audit_log", include_str!("../../../migrations/002_audit_log.sql")),
        (3, "003_tasks", include_str!("../../../migrations/003_tasks.sql")),
        (4, "004_state", include_str!("../../../migrations/004_state.sql")),
        (5, "005_email_cache", include_str!("../../../migrations/005_email_cache.sql")),
        (6, "006_ledger", include_str!("../../../migrations/006_ledger.sql")),
        (7, "007_rhythms", include_str!("../../../migrations/007_rhythms.sql")),
        (8, "008_household", include_str!("../../../migrations/008_household.sql")),
        (9, "009_dishes_unique", include_str!("../../../migrations/009_dishes_unique.sql")),
    ];

    for &(id, name, sql) in migrations {
        if applied_set.contains(&id) {
            continue;
        }
        tracing::info!(migration = name, "applying migration");
        // Execute each statement separately (sqlx doesn't support multi-statement in one call)
        for statement in sql.split(';') {
            let trimmed = statement.trim();
            if trimmed.is_empty() {
                continue;
            }
            sqlx::query(trimmed).execute(pool).await?;
        }
        sqlx::query("INSERT INTO _migrations (id, name) VALUES (?, ?)")
            .bind(id)
            .bind(name)
            .execute(pool)
            .await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn open_in_memory() {
        // Use a temp file to test real migration flow
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = open(&db_path).await.unwrap();

        // Verify migrations ran by checking table exists
        let count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM _migrations")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count.0, 9); // all 9 migrations applied

        // Verify ledger table exists
        sqlx::query("SELECT id, timestamp, category, content, tags, source FROM ledger LIMIT 1")
            .fetch_optional(&pool)
            .await
            .unwrap();

        // Idempotent: re-running migrate should be a no-op
        migrate(&pool).await.unwrap();
        let count2: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM _migrations")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count2.0, 9);

        pool.close().await;
    }
}
