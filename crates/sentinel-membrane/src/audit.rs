use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sentinel_core::capability::{Capability, CapabilityKind};
use sentinel_core::types::{ActionSource, Decision, TokenCost};

/// Unique audit entry identifier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditId(pub uuid::Uuid);

/// A single audit log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: AuditId,
    pub timestamp: DateTime<Utc>,
    pub capability_kind: CapabilityKind,
    pub capability_json: String,
    pub source: ActionSource,
    pub decision: Decision,
    pub cortex_reasoning: String,
    pub execution_result: Option<String>,
    pub token_cost: Option<TokenCost>,
}

impl AuditEntry {
    pub fn new(
        capability: &Capability,
        source: ActionSource,
        decision: Decision,
        cortex_reasoning: String,
    ) -> Self {
        Self {
            id: AuditId(uuid::Uuid::new_v4()),
            timestamp: Utc::now(),
            capability_kind: capability.kind(),
            capability_json: serde_json::to_string(capability).unwrap_or_default(),
            source,
            decision,
            cortex_reasoning,
            execution_result: None,
            token_cost: None,
        }
    }

    pub fn with_result(mut self, result: String) -> Self {
        self.execution_result = Some(result);
        self
    }

    pub fn with_token_cost(mut self, cost: TokenCost) -> Self {
        self.token_cost = Some(cost);
        self
    }
}

/// Audit logger backed by SQLite.
pub struct AuditLog {
    pool: sqlx::SqlitePool,
}

impl AuditLog {
    pub fn new(pool: sqlx::SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn record(&self, entry: &AuditEntry) -> anyhow::Result<()> {
        // Always log via tracing
        tracing::info!(
            audit_id = %entry.id.0,
            capability = %entry.capability_kind,
            decision = %entry.decision,
            reasoning = %entry.cortex_reasoning,
            "audit"
        );

        let id = entry.id.0.to_string();
        let timestamp = entry.timestamp.to_rfc3339();
        let capability_type = format!("{:?}", entry.capability_kind);
        let source = format!("{:?}", entry.source);
        let decision = format!("{}", entry.decision);
        let (input_tokens, output_tokens, cached_tokens) = entry
            .token_cost
            .as_ref()
            .map(|c| (c.input_tokens as i64, c.output_tokens as i64, c.cached_tokens as i64))
            .unwrap_or((0, 0, 0));

        sqlx::query(
            "INSERT INTO audit_log
             (id, timestamp, capability_type, capability_data, source, decision,
              cortex_reasoning, execution_result, input_tokens, output_tokens, cached_tokens)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&timestamp)
        .bind(&capability_type)
        .bind(&entry.capability_json)
        .bind(&source)
        .bind(&decision)
        .bind(&entry.cortex_reasoning)
        .bind(&entry.execution_result)
        .bind(input_tokens)
        .bind(output_tokens)
        .bind(cached_tokens)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Count entries by decision in the current hour.
    pub async fn count_recent(&self, decision: &Decision) -> anyhow::Result<u32> {
        let cutoff = Utc::now() - chrono::Duration::hours(1);
        let cutoff_str = cutoff.to_rfc3339();
        let decision_str = format!("{}", decision);

        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM audit_log WHERE decision = ? AND timestamp >= ?",
        )
        .bind(&decision_str)
        .bind(&cutoff_str)
        .fetch_one(&self.pool)
        .await?;

        Ok(row.0 as u32)
    }

    /// Purge audit entries older than N days. Returns number deleted.
    pub async fn purge_older_than(&self, days: u32) -> anyhow::Result<u64> {
        let cutoff = Utc::now() - chrono::Duration::days(days as i64);
        let cutoff_str = cutoff.to_rfc3339();
        let result = sqlx::query("DELETE FROM audit_log WHERE timestamp < ?")
            .bind(&cutoff_str)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// Sum token costs since a given time.
    pub async fn total_cost_since(&self, since: DateTime<Utc>) -> anyhow::Result<TokenCost> {
        let since_str = since.to_rfc3339();
        let row: (i64, i64, i64) = sqlx::query_as(
            "SELECT COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0), COALESCE(SUM(cached_tokens), 0)
             FROM audit_log WHERE timestamp >= ?"
        )
        .bind(&since_str)
        .fetch_one(&self.pool)
        .await?;
        Ok(TokenCost {
            input_tokens: row.0 as u32,
            output_tokens: row.1 as u32,
            cached_tokens: row.2 as u32,
        })
    }

    /// Retrieve recent rejections (HumanRejected or HumanModified) within N days.
    pub async fn recent_rejections(&self, days: u32) -> anyhow::Result<Vec<AuditEntry>> {
        let cutoff = Utc::now() - chrono::Duration::days(days as i64);
        let cutoff_str = cutoff.to_rfc3339();
        let rejected = format!("{}", Decision::HumanRejected);
        let modified = format!("{}", Decision::HumanModified);

        let rows: Vec<(String, String, String, String, String, String, String, Option<String>, i64, i64, i64)> =
            sqlx::query_as(
                "SELECT id, timestamp, capability_type, capability_data, source, decision,
                        cortex_reasoning, execution_result, input_tokens, output_tokens, cached_tokens
                 FROM audit_log WHERE (decision = ? OR decision = ?) AND timestamp >= ?
                 ORDER BY timestamp DESC",
            )
            .bind(&rejected)
            .bind(&modified)
            .bind(&cutoff_str)
            .fetch_all(&self.pool)
            .await?;

        rows.into_iter()
            .map(|(id, timestamp, cap_type, cap_data, _source, decision, reasoning, exec_result, inp, out, cached)| {
                let uuid = uuid::Uuid::parse_str(&id)?;
                let ts = DateTime::parse_from_rfc3339(&timestamp)?.with_timezone(&Utc);
                let cap_kind: CapabilityKind = serde_json::from_value(
                    serde_json::Value::String(cap_type),
                ).unwrap_or(CapabilityKind::EmailRead);
                let dec: Decision = serde_json::from_value(
                    serde_json::Value::String(decision),
                ).unwrap_or(Decision::ParseFailed);

                Ok(AuditEntry {
                    id: AuditId(uuid),
                    timestamp: ts,
                    capability_kind: cap_kind,
                    capability_json: cap_data,
                    source: ActionSource::Cortex,
                    decision: dec,
                    cortex_reasoning: reasoning,
                    execution_result: exec_result,
                    token_cost: Some(TokenCost {
                        input_tokens: inp as u32,
                        output_tokens: out as u32,
                        cached_tokens: cached as u32,
                    }),
                })
            })
            .collect()
    }

    /// Retrieve the most recent N audit entries, newest first.
    pub async fn recent(&self, limit: u32) -> anyhow::Result<Vec<AuditEntry>> {
        let rows: Vec<(String, String, String, String, String, String, String, Option<String>, i64, i64, i64)> =
            sqlx::query_as(
                "SELECT id, timestamp, capability_type, capability_data, source, decision,
                        cortex_reasoning, execution_result, input_tokens, output_tokens, cached_tokens
                 FROM audit_log ORDER BY timestamp DESC LIMIT ?",
            )
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;

        rows.into_iter()
            .map(|(id, timestamp, cap_type, cap_data, _source, decision, reasoning, exec_result, inp, out, cached)| {
                let uuid = uuid::Uuid::parse_str(&id)?;
                let ts = DateTime::parse_from_rfc3339(&timestamp)?.with_timezone(&Utc);
                let cap_kind: CapabilityKind = serde_json::from_value(
                    serde_json::Value::String(cap_type),
                ).unwrap_or(CapabilityKind::EmailRead);
                let dec: Decision = serde_json::from_value(
                    serde_json::Value::String(decision),
                ).unwrap_or(Decision::ParseFailed);

                Ok(AuditEntry {
                    id: AuditId(uuid),
                    timestamp: ts,
                    capability_kind: cap_kind,
                    capability_json: cap_data,
                    source: ActionSource::Cortex,
                    decision: dec,
                    cortex_reasoning: reasoning,
                    execution_result: exec_result,
                    token_cost: Some(TokenCost {
                        input_tokens: inp as u32,
                        output_tokens: out as u32,
                        cached_tokens: cached as u32,
                    }),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_core::capability::Capability;

    async fn test_audit() -> (AuditLog, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = sentinel_memory::db::open(&db_path).await.unwrap();
        (AuditLog::new(pool), dir)
    }

    #[tokio::test]
    async fn record_and_retrieve() {
        let (audit, _dir) = test_audit().await;

        let entry = AuditEntry::new(
            &Capability::TaskListRead,
            ActionSource::Cortex,
            Decision::AutoApproved,
            "User asked for task list".into(),
        )
        .with_token_cost(TokenCost {
            input_tokens: 500,
            output_tokens: 120,
            cached_tokens: 200,
        });

        audit.record(&entry).await.unwrap();

        let recent = audit.recent(10).await.unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].decision, Decision::AutoApproved);
        assert_eq!(recent[0].capability_kind, CapabilityKind::TaskListRead);
        assert_eq!(recent[0].token_cost.as_ref().unwrap().input_tokens, 500);
    }

    #[tokio::test]
    async fn count_recent_by_decision() {
        let (audit, _dir) = test_audit().await;

        for _ in 0..3 {
            let entry = AuditEntry::new(
                &Capability::TaskListRead,
                ActionSource::Cortex,
                Decision::AutoApproved,
                "auto".into(),
            );
            audit.record(&entry).await.unwrap();
        }

        let entry = AuditEntry::new(
            &Capability::TaskListRead,
            ActionSource::Cortex,
            Decision::PolicyBlocked,
            "blocked".into(),
        );
        audit.record(&entry).await.unwrap();

        let approved = audit.count_recent(&Decision::AutoApproved).await.unwrap();
        assert_eq!(approved, 3);

        let blocked = audit.count_recent(&Decision::PolicyBlocked).await.unwrap();
        assert_eq!(blocked, 1);
    }

    #[tokio::test]
    async fn total_cost_since_aggregates_tokens() {
        let (audit, _dir) = test_audit().await;

        for i in 0..3u32 {
            let entry = AuditEntry::new(
                &Capability::TaskListRead,
                ActionSource::Cortex,
                Decision::AutoApproved,
                "auto".into(),
            )
            .with_token_cost(TokenCost {
                input_tokens: 100 * (i + 1),
                output_tokens: 50 * (i + 1),
                cached_tokens: 10 * (i + 1),
            });
            audit.record(&entry).await.unwrap();
        }

        let cost = audit
            .total_cost_since(Utc::now() - chrono::Duration::hours(1))
            .await
            .unwrap();
        // Sum: input 100+200+300=600, output 50+100+150=300, cached 10+20+30=60
        assert_eq!(cost.input_tokens, 600);
        assert_eq!(cost.output_tokens, 300);
        assert_eq!(cost.cached_tokens, 60);
    }

    #[tokio::test]
    async fn recent_rejections_filters_correctly() {
        let (audit, _dir) = test_audit().await;

        // Record a rejection
        let rejected = AuditEntry::new(
            &Capability::TaskListRead,
            ActionSource::Cortex,
            Decision::HumanRejected,
            "user disagreed with task list action".into(),
        );
        audit.record(&rejected).await.unwrap();

        // Record a modification
        let modified = AuditEntry::new(
            &Capability::TaskListRead,
            ActionSource::Cortex,
            Decision::HumanModified,
            "user edited the draft".into(),
        );
        audit.record(&modified).await.unwrap();

        // Record an approval (should NOT appear in rejections)
        let approved = AuditEntry::new(
            &Capability::TaskListRead,
            ActionSource::Cortex,
            Decision::AutoApproved,
            "auto".into(),
        );
        audit.record(&approved).await.unwrap();

        let rejections = audit.recent_rejections(7).await.unwrap();
        assert_eq!(rejections.len(), 2);
        // Both should be correction decisions
        assert!(rejections.iter().all(|r| r.decision == Decision::HumanRejected
            || r.decision == Decision::HumanModified));
    }
}
