use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

/// The append-only event ledger. Records everything, zero AI cost.
pub struct Ledger {
    pool: SqlitePool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub id: LedgerId,
    pub timestamp: DateTime<Utc>,
    pub category: LedgerCategory,
    pub content: String,
    pub tags: Vec<String>,
    pub source: LedgerSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerId(pub uuid::Uuid);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LedgerCategory {
    // System-generated
    MealCooked,
    ShoppingTrip,
    TaskCompleted,
    TaskSkipped,
    EmailReceived,
    EmailActedOn,
    AppointmentKept,
    AppointmentMissed,
    DepartureAlert,
    EventAttended,

    // User-generated
    UserNote,
    UserTracking,
    UserInterest,

    // AI-generated (from reflections)
    Observation,
    Inference,
}

impl std::fmt::Display for LedgerCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = serde_json::to_value(self)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| format!("{self:?}"));
        f.write_str(&s)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LedgerSource {
    Watcher(String),
    User,
    Cortex,
    System,
}

impl std::fmt::Display for LedgerSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LedgerSource::Watcher(name) => write!(f, "watcher:{name}"),
            LedgerSource::User => write!(f, "user"),
            LedgerSource::Cortex => write!(f, "cortex"),
            LedgerSource::System => write!(f, "system"),
        }
    }
}

impl Ledger {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Create a new ledger entry with a fresh UUID and current timestamp.
    pub fn entry(
        category: LedgerCategory,
        content: String,
        source: LedgerSource,
    ) -> LedgerEntry {
        LedgerEntry {
            id: LedgerId(uuid::Uuid::new_v4()),
            timestamp: Utc::now(),
            category,
            content,
            tags: vec![],
            source,
        }
    }

    /// Append an entry to the ledger.
    pub async fn append(&self, entry: &LedgerEntry) -> anyhow::Result<()> {
        let id = entry.id.0.to_string();
        let timestamp = entry.timestamp.to_rfc3339();
        let category = entry.category.to_string();
        let tags = serde_json::to_string(&entry.tags)?;
        let source = entry.source.to_string();

        sqlx::query(
            "INSERT INTO ledger (id, timestamp, category, content, tags, source)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&timestamp)
        .bind(&category)
        .bind(&entry.content)
        .bind(&tags)
        .bind(&source)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Retrieve the most recent N entries, newest first.
    pub async fn recent(&self, limit: u32) -> anyhow::Result<Vec<LedgerEntry>> {
        let rows: Vec<(String, String, String, String, String, String)> = sqlx::query_as(
            "SELECT id, timestamp, category, content, tags, source
             FROM ledger ORDER BY timestamp DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(|r| row_to_entry(r)).collect()
    }

    /// Retrieve entries from the last N hours, newest first.
    pub async fn recent_hours(&self, hours: u32) -> anyhow::Result<Vec<LedgerEntry>> {
        let cutoff = Utc::now() - chrono::Duration::hours(hours as i64);
        let cutoff_str = cutoff.to_rfc3339();

        let rows: Vec<(String, String, String, String, String, String)> = sqlx::query_as(
            "SELECT id, timestamp, category, content, tags, source
             FROM ledger WHERE timestamp >= ? ORDER BY timestamp DESC",
        )
        .bind(&cutoff_str)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(|r| row_to_entry(r)).collect()
    }

    /// Search entries by content substring.
    pub async fn search(&self, query: &str, limit: u32) -> anyhow::Result<Vec<LedgerEntry>> {
        let pattern = format!("%{query}%");
        let rows: Vec<(String, String, String, String, String, String)> = sqlx::query_as(
            "SELECT id, timestamp, category, content, tags, source
             FROM ledger WHERE content LIKE ? ORDER BY timestamp DESC LIMIT ?",
        )
        .bind(&pattern)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(|r| row_to_entry(r)).collect()
    }

    pub async fn search_since(&self, query: &str, limit: u32, since: DateTime<Utc>) -> anyhow::Result<Vec<LedgerEntry>> {
        let pattern = format!("%{query}%");
        let since_str = since.to_rfc3339();
        let rows: Vec<(String, String, String, String, String, String)> = sqlx::query_as(
            "SELECT id, timestamp, category, content, tags, source
             FROM ledger WHERE content LIKE ? AND timestamp >= ? ORDER BY timestamp DESC LIMIT ?",
        )
        .bind(&pattern)
        .bind(&since_str)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(|r| row_to_entry(r)).collect()
    }

    /// Count entries by category.
    pub async fn count_by_category(&self, category: &LedgerCategory) -> anyhow::Result<i64> {
        let cat_str = category.to_string();
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM ledger WHERE category = ?",
        )
        .bind(&cat_str)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    /// Total number of entries in the ledger.
    pub async fn count(&self) -> anyhow::Result<i64> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM ledger")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0)
    }

    /// Purge entries older than N days. Returns number deleted.
    pub async fn purge_older_than(&self, days: u32) -> anyhow::Result<u64> {
        let cutoff = Utc::now() - chrono::Duration::days(days as i64);
        let cutoff_str = cutoff.to_rfc3339();
        let result = sqlx::query("DELETE FROM ledger WHERE timestamp < ?")
            .bind(&cutoff_str)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }
}

fn row_to_entry(
    row: (String, String, String, String, String, String),
) -> anyhow::Result<LedgerEntry> {
    let (id, timestamp, category, content, tags, source) = row;
    let uuid = uuid::Uuid::parse_str(&id)?;
    let ts = DateTime::parse_from_rfc3339(&timestamp)?.with_timezone(&Utc);
    let cat: LedgerCategory = serde_json::from_value(serde_json::Value::String(category))?;
    let tag_vec: Vec<String> = serde_json::from_str(&tags)?;
    let src = parse_source(&source);

    Ok(LedgerEntry {
        id: LedgerId(uuid),
        timestamp: ts,
        category: cat,
        content,
        tags: tag_vec,
        source: src,
    })
}

fn parse_source(s: &str) -> LedgerSource {
    if let Some(name) = s.strip_prefix("watcher:") {
        LedgerSource::Watcher(name.to_string())
    } else {
        match s {
            "user" => LedgerSource::User,
            "cortex" => LedgerSource::Cortex,
            _ => LedgerSource::System,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_ledger() -> (Ledger, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = crate::db::open(&db_path).await.unwrap();
        (Ledger::new(pool), dir)
    }

    #[tokio::test]
    async fn append_and_retrieve() {
        let (ledger, _dir) = test_ledger().await;

        let entry = Ledger::entry(
            LedgerCategory::EmailReceived,
            "From: ana@example.com — Dinner tomorrow?".into(),
            LedgerSource::Watcher("email".into()),
        );
        ledger.append(&entry).await.unwrap();

        let recent = ledger.recent(10).await.unwrap();
        assert_eq!(recent.len(), 1);
        assert!(recent[0].content.contains("Dinner tomorrow"));

        let count = ledger.count().await.unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn search_by_content() {
        let (ledger, _dir) = test_ledger().await;

        ledger
            .append(&Ledger::entry(
                LedgerCategory::MealCooked,
                "Made fish stew for dinner".into(),
                LedgerSource::User,
            ))
            .await
            .unwrap();

        ledger
            .append(&Ledger::entry(
                LedgerCategory::TaskCompleted,
                "Paid electricity bill".into(),
                LedgerSource::System,
            ))
            .await
            .unwrap();

        let results = ledger.search("fish", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("fish stew"));

        let results = ledger.search("electricity", 10).await.unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn count_by_category() {
        let (ledger, _dir) = test_ledger().await;

        for i in 0..3 {
            ledger
                .append(&Ledger::entry(
                    LedgerCategory::EmailReceived,
                    format!("Email {i}"),
                    LedgerSource::Watcher("email".into()),
                ))
                .await
                .unwrap();
        }

        ledger
            .append(&Ledger::entry(
                LedgerCategory::UserNote,
                "A user note".into(),
                LedgerSource::User,
            ))
            .await
            .unwrap();

        let email_count = ledger
            .count_by_category(&LedgerCategory::EmailReceived)
            .await
            .unwrap();
        assert_eq!(email_count, 3);

        let note_count = ledger
            .count_by_category(&LedgerCategory::UserNote)
            .await
            .unwrap();
        assert_eq!(note_count, 1);
    }
}

