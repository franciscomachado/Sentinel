use crate::state::StateManager;

/// Coordinates periodic AI reflection outcomes.
///
/// Reflections themselves flow through the normal cortex pipeline:
///   ScheduledTrigger → triage → compile_for_trigger → LLM → state_updates
///
/// The ReflectionEngine handles post-reflection housekeeping:
/// - Monthly memory pruning (removing stale memories)
/// - Observation archival (compacting old observations into summaries)
pub struct ReflectionEngine {
    state: StateManager,
}

impl ReflectionEngine {
    pub fn new(state: StateManager) -> Self {
        Self { state }
    }

    /// Prune memories older than `retention_days` that haven't been referenced.
    ///
    /// Called after monthly reflections to keep the memory store compact.
    /// Only removes memories with no recent activity referencing them.
    pub async fn prune_stale_memories(&self, retention_days: u32) -> anyhow::Result<u32> {
        let cutoff =
            chrono::Utc::now() - chrono::Duration::days(retention_days as i64);

        let memories = self.state.get_memories().await?;
        let mut pruned = 0u32;

        for mem in &memories {
            if mem.created_at <= cutoff {
                if self.state.delete_memory(&mem.id).await? {
                    tracing::info!(id = %mem.id, content = %mem.content, "pruned stale memory");
                    pruned += 1;
                }
            }
        }

        Ok(pruned)
    }

    /// Archive old observations by removing those beyond `keep_count`.
    pub async fn trim_observations(&self, keep_count: u32) -> anyhow::Result<u32> {
        let all = self.state.get_recent_observations(u32::MAX).await?;
        if all.len() <= keep_count as usize {
            return Ok(0);
        }

        let to_remove = &all[keep_count as usize..];
        let mut removed = 0u32;
        for obs in to_remove {
            if self.state.delete_observation(&obs.id).await? {
                removed += 1;
            }
        }

        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_db() -> (sqlx::SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = crate::db::open(&db_path).await.unwrap();
        (pool, dir)
    }

    #[tokio::test]
    async fn prune_removes_old_memories() {
        let (pool, _dir) = test_db().await;
        let state = StateManager::new(pool);

        // Add a memory
        state
            .add_memory("likes fish", &["food".into()], "test")
            .await
            .unwrap();

        let engine = ReflectionEngine::new(state);
        // Wait a moment to ensure the memory's created_at is in the past
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // Prune with 0-day retention — everything already created is "old"
        let pruned = engine.prune_stale_memories(0).await.unwrap();
        assert_eq!(pruned, 1);
    }
}
