use chrono::{DateTime, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

/// Parse a timestamp string from SQLite, handling both RFC3339 and SQLite's datetime format.
fn parse_sqlite_datetime(s: &str) -> DateTime<Utc> {
    // Try RFC3339 first
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return dt.with_timezone(&Utc);
    }
    // Try SQLite's datetime format: "2025-01-15 12:34:56"
    if let Ok(naive) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return naive.and_utc();
    }
    Utc::now()
}

/// Persistent state manager for watcher state, memories, and observations.
#[derive(Clone)]
pub struct StateManager {
    pool: SqlitePool,
}

/// Key-value state for individual watchers (e.g., IMAP UID highwater mark).
#[derive(Debug, Clone)]
pub struct WatcherState {
    pub watcher_id: String,
    pub state_key: String,
    pub state_value: String,
}

/// A memory entry — persisted knowledge that informs future context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub content: String,
    pub tags: Vec<String>,
    pub source: String,
    pub created_at: DateTime<Utc>,
}

/// An observation from a reflection — internal AI-generated insight.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub id: String,
    pub content: String,
    pub source: String,
    pub created_at: DateTime<Utc>,
}

impl StateManager {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Access the underlying pool (for testing / advanced queries).
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    // --- Watcher state ---

    /// Get a watcher's persisted state value.
    pub async fn get_watcher_state(
        &self,
        watcher_id: &str,
        key: &str,
    ) -> anyhow::Result<Option<String>> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT state_value FROM watcher_state WHERE watcher_id = ? AND state_key = ?",
        )
        .bind(watcher_id)
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| r.0))
    }

    /// Set a watcher's persisted state value (upsert).
    pub async fn set_watcher_state(
        &self,
        watcher_id: &str,
        key: &str,
        value: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO watcher_state (watcher_id, state_key, state_value, updated_at)
             VALUES (?, ?, ?, datetime('now'))
             ON CONFLICT(watcher_id, state_key) DO UPDATE SET
                state_value = excluded.state_value,
                updated_at = excluded.updated_at",
        )
        .bind(watcher_id)
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // --- Memories ---

    /// Store a new memory entry.
    pub async fn add_memory(
        &self,
        content: &str,
        tags: &[String],
        source: &str,
    ) -> anyhow::Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let tags_json = serde_json::to_string(tags)?;

        sqlx::query(
            "INSERT INTO memories (id, content, tags, source) VALUES (?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(content)
        .bind(&tags_json)
        .bind(source)
        .execute(&self.pool)
        .await?;

        Ok(id)
    }

    /// Retrieve all active memories (for state compiler context).
    pub async fn get_memories(&self) -> anyhow::Result<Vec<MemoryEntry>> {
        let rows: Vec<(String, String, String, String, String)> = sqlx::query_as(
            "SELECT id, content, tags, source, created_at FROM memories ORDER BY created_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|(id, content, tags, source, created_at)| {
                let tag_vec: Vec<String> = serde_json::from_str(&tags).unwrap_or_default();
                let ts = parse_sqlite_datetime(&created_at);
                Ok(MemoryEntry {
                    id,
                    content,
                    tags: tag_vec,
                    source,
                    created_at: ts,
                })
            })
            .collect()
    }

    /// Search memories by content substring.
    pub async fn search_memories(
        &self,
        query: &str,
        limit: u32,
    ) -> anyhow::Result<Vec<MemoryEntry>> {
        let pattern = format!("%{query}%");
        let rows: Vec<(String, String, String, String, String)> = sqlx::query_as(
            "SELECT id, content, tags, source, created_at FROM memories
             WHERE content LIKE ?
             ORDER BY created_at DESC LIMIT ?",
        )
        .bind(&pattern)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|(id, content, tags, source, created_at)| {
                let tag_vec: Vec<String> = serde_json::from_str(&tags).unwrap_or_default();
                let ts = parse_sqlite_datetime(&created_at);
                Ok(MemoryEntry {
                    id,
                    content,
                    tags: tag_vec,
                    source,
                    created_at: ts,
                })
            })
            .collect()
    }

    /// Delete a memory by ID.
    pub async fn delete_memory(&self, id: &str) -> anyhow::Result<bool> {
        let result = sqlx::query("DELETE FROM memories WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // --- Observations ---

    /// Store an observation from a reflection.
    pub async fn add_observation(
        &self,
        content: &str,
        source: &str,
    ) -> anyhow::Result<String> {
        let id = uuid::Uuid::new_v4().to_string();

        sqlx::query(
            "INSERT INTO observations (id, content, source) VALUES (?, ?, ?)",
        )
        .bind(&id)
        .bind(content)
        .bind(source)
        .execute(&self.pool)
        .await?;

        Ok(id)
    }

    /// Retrieve recent observations (not superseded).
    pub async fn get_recent_observations(
        &self,
        limit: u32,
    ) -> anyhow::Result<Vec<Observation>> {
        let rows: Vec<(String, String, String, String)> = sqlx::query_as(
            "SELECT id, content, source, created_at FROM observations
             WHERE superseded_by IS NULL
             ORDER BY created_at DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|(id, content, source, created_at)| {
                let ts = parse_sqlite_datetime(&created_at);
                Ok(Observation {
                    id,
                    content,
                    source,
                    created_at: ts,
                })
            })
            .collect()
    }

    /// Delete an observation by ID.
    pub async fn delete_observation(&self, id: &str) -> anyhow::Result<bool> {
        let result = sqlx::query("DELETE FROM observations WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // --- Travel mode ---

    /// Get the current travel mode (if any).
    pub async fn get_travel_mode(&self) -> anyhow::Result<Option<sentinel_core::types::TravelMode>> {
        let val = self.get_watcher_state("system", "travel_mode").await?;
        match val {
            Some(json) => {
                let mode: sentinel_core::types::TravelMode = serde_json::from_str(&json)?;
                if mode.is_currently_active() {
                    Ok(Some(mode))
                } else {
                    Ok(None)
                }
            }
            None => Ok(None),
        }
    }

    /// Set or update travel mode.
    pub async fn set_travel_mode(&self, mode: &sentinel_core::types::TravelMode) -> anyhow::Result<()> {
        let json = serde_json::to_string(mode)?;
        self.set_watcher_state("system", "travel_mode", &json).await
    }

    /// Clear travel mode (return to normal).
    pub async fn clear_travel_mode(&self) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM watcher_state WHERE watcher_id = 'system' AND state_key = 'travel_mode'")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // --- Engagement tracking ---

    /// Record a user interaction (ack, query, etc.) to track engagement.
    pub async fn record_interaction(&self) -> anyhow::Result<()> {
        let now = Utc::now().to_rfc3339();
        self.set_watcher_state("system", "last_interaction", &now).await
    }

    /// Compute the current engagement level based on last interaction.
    pub async fn engagement_level(&self) -> anyhow::Result<sentinel_core::types::EngagementLevel> {
        use sentinel_core::types::EngagementLevel;
        let val = self.get_watcher_state("system", "last_interaction").await?;
        match val {
            Some(ts_str) => {
                let ts = parse_sqlite_datetime(&ts_str);
                let hours_since = (Utc::now() - ts).num_hours();
                if hours_since < 48 {
                    Ok(EngagementLevel::Active)
                } else if hours_since < 120 { // 5 days
                    Ok(EngagementLevel::Quiet)
                } else {
                    Ok(EngagementLevel::Absent)
                }
            }
            None => Ok(EngagementLevel::Active), // no data yet → assume active
        }
    }

    // --- Export ---

    /// Export all user data as JSON (GDPR-style).
    pub async fn export_all(&self) -> anyhow::Result<serde_json::Value> {
        let memories = self.get_memories().await?;
        let observations = self.get_recent_observations(1000).await?;
        let travel = self.get_travel_mode().await?;

        Ok(serde_json::json!({
            "memories": memories,
            "observations": observations,
            "travel_mode": travel,
        }))
    }

    /// Count all memories.
    pub async fn count_memories(&self) -> anyhow::Result<i64> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM memories")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0)
    }

    /// Delete memories by tag.
    pub async fn delete_memories_by_tag(&self, tag: &str) -> anyhow::Result<u64> {
        let pattern = format!("%\"{tag}\"%");
        let result = sqlx::query("DELETE FROM memories WHERE tags LIKE ?")
            .bind(&pattern)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// Delete memories older than N days.
    pub async fn delete_memories_before(&self, days: u32) -> anyhow::Result<u64> {
        let cutoff = Utc::now() - chrono::Duration::days(days as i64);
        let cutoff_str = cutoff.to_rfc3339();
        let result = sqlx::query("DELETE FROM memories WHERE created_at < ?")
            .bind(&cutoff_str)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_state() -> (StateManager, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = crate::db::open(&db_path).await.unwrap();
        (StateManager::new(pool), dir)
    }

    #[tokio::test]
    async fn watcher_state_round_trip() {
        let (sm, _dir) = test_state().await;

        // Initially empty
        let val = sm.get_watcher_state("email", "uid_highwater").await.unwrap();
        assert!(val.is_none());

        // Set and get
        sm.set_watcher_state("email", "uid_highwater", "42")
            .await
            .unwrap();
        let val = sm.get_watcher_state("email", "uid_highwater").await.unwrap();
        assert_eq!(val.as_deref(), Some("42"));

        // Update
        sm.set_watcher_state("email", "uid_highwater", "99")
            .await
            .unwrap();
        let val = sm.get_watcher_state("email", "uid_highwater").await.unwrap();
        assert_eq!(val.as_deref(), Some("99"));
    }

    #[tokio::test]
    async fn memory_crud() {
        let (sm, _dir) = test_state().await;

        let id = sm
            .add_memory(
                "Kids don't like fish stew",
                &["food".into(), "preference".into()],
                "weekly_reflection",
            )
            .await
            .unwrap();

        sm.add_memory(
            "Blood pressure averaging 136/80",
            &["health".into()],
            "user",
        )
        .await
        .unwrap();

        // List all
        let memories = sm.get_memories().await.unwrap();
        assert_eq!(memories.len(), 2);

        // Search
        let results = sm.search_memories("fish", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("fish stew"));

        // Delete
        let deleted = sm.delete_memory(&id).await.unwrap();
        assert!(deleted);
        let memories = sm.get_memories().await.unwrap();
        assert_eq!(memories.len(), 1);
    }

    #[tokio::test]
    async fn observations() {
        let (sm, _dir) = test_state().await;

        sm.add_observation(
            "John tends to forget bills on busy weeks",
            "weekly_reflection",
        )
        .await
        .unwrap();

        sm.add_observation(
            "Morning briefings are read within 5 minutes",
            "monthly_reflection",
        )
        .await
        .unwrap();

        let obs = sm.get_recent_observations(10).await.unwrap();
        assert_eq!(obs.len(), 2);
    }

    #[tokio::test]
    async fn travel_mode_lifecycle() {
        let (sm, _dir) = test_state().await;

        // Initially no travel mode
        assert!(sm.get_travel_mode().await.unwrap().is_none());

        // Set travel mode with far-future dates so it's "active"
        let mode = sentinel_core::types::TravelMode {
            destination: "London".into(),
            hotel: Some("The Hoxton".into()),
            start_date: "2020-01-01".into(),
            end_date: "2099-12-31".into(),
            timezone_override: Some("Europe/London".into()),
            weather_lat: None,
            weather_lon: None,
            active: true,
        };
        sm.set_travel_mode(&mode).await.unwrap();

        let retrieved = sm.get_travel_mode().await.unwrap();
        assert!(retrieved.is_some());
        let m = retrieved.unwrap();
        assert_eq!(m.destination, "London");
        assert_eq!(m.hotel.as_deref(), Some("The Hoxton"));

        // Clear
        sm.clear_travel_mode().await.unwrap();
        assert!(sm.get_travel_mode().await.unwrap().is_none());
    }
}
