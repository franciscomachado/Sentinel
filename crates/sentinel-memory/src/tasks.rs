use chrono::{DateTime, Utc};
use sentinel_core::capability::{Task, TaskPatch};
use sentinel_core::holidays::load_calendar_for_country;
use sentinel_core::schedule::TaskSchedule;
use sentinel_core::types::Urgency;
use sqlx::SqlitePool;

/// Persistent task store backed by SQLite.
#[derive(Clone)]
pub struct TaskStore {
    pool: SqlitePool,
}

/// A stored task with DB-level fields.
#[derive(Debug, Clone)]
pub struct StoredTask {
    pub id: String,
    pub title: String,
    pub notes: Option<String>,
    pub schedule: TaskSchedule,
    pub next_trigger: Option<DateTime<Utc>>,
    pub context: Vec<String>,
    pub conditions: Vec<String>,
    pub urgency: Urgency,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
    pub completed_at: Option<DateTime<Utc>>,
}

impl TaskStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Create a new task, returning its ID.
    pub async fn create(&self, task: &Task, created_by: &str) -> anyhow::Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let (schedule_type, schedule_data) = serialize_schedule(&task.schedule)?;
        let next_trigger = compute_initial_trigger(&task.schedule);
        let next_trigger_str = next_trigger.map(|dt| dt.to_rfc3339());
        let context_json = serde_json::to_string(&task.context)?;
        let conditions_json = serde_json::to_string(&task.conditions)?;
        let urgency_str = format!("{:?}", task.urgency);

        sqlx::query(
            "INSERT INTO tasks (id, title, notes, schedule_type, schedule_data,
                               next_trigger, context, conditions, urgency, created_by)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&task.title)
        .bind(&task.notes)
        .bind(&schedule_type)
        .bind(&schedule_data)
        .bind(&next_trigger_str)
        .bind(&context_json)
        .bind(&conditions_json)
        .bind(&urgency_str)
        .bind(created_by)
        .execute(&self.pool)
        .await?;

        Ok(id)
    }

    /// Get a single task by ID.
    pub async fn get(&self, id: &str) -> anyhow::Result<Option<StoredTask>> {
        let row: Option<TaskRow> = sqlx::query_as(
            "SELECT id, title, notes, schedule_type, schedule_data, next_trigger,
                    context, conditions, urgency, created_at, created_by, completed_at
             FROM tasks WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_task).transpose()
    }

    /// List all active (non-completed) tasks.
    pub async fn list_active(&self) -> anyhow::Result<Vec<StoredTask>> {
        let rows: Vec<TaskRow> = sqlx::query_as(
            "SELECT id, title, notes, schedule_type, schedule_data, next_trigger,
                    context, conditions, urgency, created_at, created_by, completed_at
             FROM tasks WHERE completed_at IS NULL
             ORDER BY next_trigger ASC NULLS LAST",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_task).collect()
    }

    /// List tasks that are due (next_trigger <= now) and not completed.
    pub async fn list_due(&self) -> anyhow::Result<Vec<StoredTask>> {
        let now = Utc::now().to_rfc3339();
        let rows: Vec<TaskRow> = sqlx::query_as(
            "SELECT id, title, notes, schedule_type, schedule_data, next_trigger,
                    context, conditions, urgency, created_at, created_by, completed_at
             FROM tasks
             WHERE completed_at IS NULL AND next_trigger IS NOT NULL AND next_trigger <= ?
             ORDER BY next_trigger ASC",
        )
        .bind(&now)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_task).collect()
    }

    /// List tasks due today (for morning briefing context).
    pub async fn list_due_today(&self) -> anyhow::Result<Vec<StoredTask>> {
        let today_start = Utc::now().date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc();
        let today_end = today_start + chrono::Duration::days(1);
        let rows: Vec<TaskRow> = sqlx::query_as(
            "SELECT id, title, notes, schedule_type, schedule_data, next_trigger,
                    context, conditions, urgency, created_at, created_by, completed_at
             FROM tasks
             WHERE completed_at IS NULL AND next_trigger IS NOT NULL
               AND next_trigger >= ? AND next_trigger < ?
             ORDER BY next_trigger ASC",
        )
        .bind(today_start.to_rfc3339())
        .bind(today_end.to_rfc3339())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_task).collect()
    }

    /// Count active (non-completed) recurring tasks.
    pub async fn count_recurring(&self) -> anyhow::Result<u32> {
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM tasks
             WHERE completed_at IS NULL
               AND (schedule_type = 'recurring' OR schedule_type = 'business_day')",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0 as u32)
    }

    /// Complete a task. For recurring tasks, advances the next trigger.
    pub async fn complete(&self, id: &str) -> anyhow::Result<bool> {
        let task = self.get(id).await?;
        let Some(task) = task else { return Ok(false) };

        // Record completion
        sqlx::query(
            "INSERT INTO task_completions (task_id) VALUES (?)",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;

        // For one-time tasks, mark as completed
        // For recurring tasks, advance to next trigger
        match &task.schedule {
            TaskSchedule::Once { .. } => {
                sqlx::query("UPDATE tasks SET completed_at = datetime('now') WHERE id = ?")
                    .bind(id)
                    .execute(&self.pool)
                    .await?;
            }
            TaskSchedule::Recurring { .. } | TaskSchedule::BusinessDay { .. } => {
                let next = advance_trigger(&task.schedule);
                let next_str = next.map(|dt| dt.to_rfc3339());
                sqlx::query("UPDATE tasks SET next_trigger = ? WHERE id = ?")
                    .bind(&next_str)
                    .bind(id)
                    .execute(&self.pool)
                    .await?;
            }
            _ => {
                // Triggered/RelativeToEvent — no automatic advancement
                sqlx::query("UPDATE tasks SET completed_at = datetime('now') WHERE id = ?")
                    .bind(id)
                    .execute(&self.pool)
                    .await?;
            }
        }

        Ok(true)
    }

    /// Update a task with a patch.
    pub async fn update(&self, id: &str, patch: &TaskPatch) -> anyhow::Result<bool> {
        let task = self.get(id).await?;
        let Some(_task) = task else { return Ok(false) };

        if let Some(ref title) = patch.title {
            sqlx::query("UPDATE tasks SET title = ? WHERE id = ?")
                .bind(title)
                .bind(id)
                .execute(&self.pool)
                .await?;
        }
        if let Some(ref notes) = patch.notes {
            sqlx::query("UPDATE tasks SET notes = ? WHERE id = ?")
                .bind(notes)
                .bind(id)
                .execute(&self.pool)
                .await?;
        }
        if let Some(ref schedule) = patch.schedule {
            let (stype, sdata) = serialize_schedule(schedule)?;
            let next = compute_initial_trigger(schedule);
            let next_str = next.map(|dt| dt.to_rfc3339());
            sqlx::query(
                "UPDATE tasks SET schedule_type = ?, schedule_data = ?, next_trigger = ? WHERE id = ?",
            )
            .bind(&stype)
            .bind(&sdata)
            .bind(&next_str)
            .bind(id)
            .execute(&self.pool)
            .await?;
        }
        if let Some(ref urgency) = patch.urgency {
            sqlx::query("UPDATE tasks SET urgency = ? WHERE id = ?")
                .bind(format!("{:?}", urgency))
                .bind(id)
                .execute(&self.pool)
                .await?;
        }

        Ok(true)
    }

    /// Format active tasks as a summary string for state compiler context.
    pub async fn summary_for_today(&self) -> String {
        let tasks = match self.list_due_today().await {
            Ok(t) => t,
            Err(_) => return String::new(),
        };
        if tasks.is_empty() {
            return String::new();
        }
        let mut lines = Vec::with_capacity(tasks.len());
        for t in &tasks {
            let ctx = if t.context.is_empty() {
                String::new()
            } else {
                format!(" ({})", t.context.join(", "))
            };
            lines.push(format!("- {}{ctx}", t.title));
        }
        lines.join("\n")
    }

    /// Format all active tasks as summary for broader context.
    pub async fn summary_active(&self) -> String {
        let tasks = match self.list_active().await {
            Ok(t) => t,
            Err(_) => return String::new(),
        };
        if tasks.is_empty() {
            return String::new();
        }
        let mut lines = Vec::with_capacity(tasks.len());
        for t in &tasks {
            let due = t
                .next_trigger
                .map(|dt| format!(" (due {})", dt.format("%Y-%m-%d")))
                .unwrap_or_default();
            lines.push(format!("- {}{due}", t.title));
        }
        lines.join("\n")
    }
}

// ── Internal helpers ────────────────────────────────────────────

type TaskRow = (
    String,         // id
    String,         // title
    Option<String>, // notes
    String,         // schedule_type
    String,         // schedule_data
    Option<String>, // next_trigger
    String,         // context
    String,         // conditions
    String,         // urgency
    String,         // created_at
    String,         // created_by
    Option<String>, // completed_at
);

fn row_to_task(row: TaskRow) -> anyhow::Result<StoredTask> {
    let (id, title, notes, schedule_type, schedule_data, next_trigger, context, conditions, urgency, created_at, created_by, completed_at) = row;

    let schedule = deserialize_schedule(&schedule_type, &schedule_data)?;
    let next_trigger = next_trigger
        .as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc));
    let context: Vec<String> = serde_json::from_str(&context).unwrap_or_default();
    let conditions: Vec<String> = serde_json::from_str(&conditions).unwrap_or_default();
    let urgency = parse_urgency(&urgency);
    let created_at = DateTime::parse_from_rfc3339(&created_at)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let completed_at = completed_at
        .as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc));

    Ok(StoredTask {
        id,
        title,
        notes,
        schedule,
        next_trigger,
        context,
        conditions,
        urgency,
        created_at,
        created_by,
        completed_at,
    })
}

fn serialize_schedule(schedule: &TaskSchedule) -> anyhow::Result<(String, String)> {
    let (stype, data) = match schedule {
        TaskSchedule::Once { due } => ("once", serde_json::json!({"due": due.to_rfc3339()})),
        TaskSchedule::Recurring { rrule } => ("recurring", serde_json::json!({"rrule": rrule})),
        TaskSchedule::BusinessDay { day_spec, holidays } => {
            ("business_day", serde_json::json!({"day_spec": day_spec, "holidays": holidays}))
        }
        TaskSchedule::Triggered { trigger } => {
            ("triggered", serde_json::json!({"condition": trigger.condition}))
        }
        TaskSchedule::RelativeToEvent {
            event_pattern,
            offset_minutes,
        } => (
            "relative",
            serde_json::json!({"event_pattern": event_pattern, "offset_minutes": offset_minutes}),
        ),
    };
    Ok((stype.to_string(), data.to_string()))
}

fn deserialize_schedule(stype: &str, data: &str) -> anyhow::Result<TaskSchedule> {
    let v: serde_json::Value = serde_json::from_str(data)?;
    match stype {
        "once" => {
            let due = v["due"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("missing 'due'"))?;
            let due = DateTime::parse_from_rfc3339(due)?.with_timezone(&Utc);
            Ok(TaskSchedule::Once { due })
        }
        "recurring" => {
            let rrule = v["rrule"]
                .as_str()
                .unwrap_or("")
                .to_string();
            Ok(TaskSchedule::Recurring { rrule })
        }
        "business_day" => {
            let day_spec = serde_json::from_value(v["day_spec"].clone())?;
            let holidays = v["holidays"].as_str().unwrap_or("").to_string();
            Ok(TaskSchedule::BusinessDay { day_spec, holidays })
        }
        "triggered" => {
            let condition = v["condition"].as_str().unwrap_or("").to_string();
            Ok(TaskSchedule::Triggered {
                trigger: sentinel_core::schedule::TaskTrigger { condition },
            })
        }
        "relative" => {
            let event_pattern = v["event_pattern"].as_str().unwrap_or("").to_string();
            let offset_minutes = v["offset_minutes"].as_i64().unwrap_or(0);
            Ok(TaskSchedule::RelativeToEvent {
                event_pattern,
                offset_minutes,
            })
        }
        other => anyhow::bail!("unknown schedule type: {other}"),
    }
}

fn compute_initial_trigger(schedule: &TaskSchedule) -> Option<DateTime<Utc>> {
    match schedule {
        TaskSchedule::Once { due } => Some(*due),
        TaskSchedule::Recurring { .. } => {
            // For RRULE, we'd need a parser. For now, set to tomorrow.
            Some(Utc::now() + chrono::Duration::days(1))
        }
        TaskSchedule::BusinessDay { day_spec, holidays } => {
            let today = Utc::now().date_naive();
            let cal = load_calendar_for_country(holidays).unwrap_or_default();
            day_spec
                .next_occurrence(today, &cal)
                .map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc())
        }
        TaskSchedule::Triggered { .. } | TaskSchedule::RelativeToEvent { .. } => None,
    }
}

fn advance_trigger(schedule: &TaskSchedule) -> Option<DateTime<Utc>> {
    match schedule {
        TaskSchedule::Recurring { rrule } => {
            // Simple RRULE parsing for common patterns:
            // FREQ=DAILY;INTERVAL=N → add N days
            // FREQ=WEEKLY;INTERVAL=N → add N weeks
            // FREQ=MONTHLY;INTERVAL=N → add N months
            let now = Utc::now();
            let freq = extract_rrule_field(rrule, "FREQ").unwrap_or_default();
            let interval: i64 = extract_rrule_field(rrule, "INTERVAL")
                .and_then(|s| s.parse().ok())
                .unwrap_or(1);

            match freq.as_str() {
                "DAILY" => Some(now + chrono::Duration::days(interval)),
                "WEEKLY" => Some(now + chrono::Duration::weeks(interval)),
                "MONTHLY" => {
                    // Approximate — add 30*interval days
                    Some(now + chrono::Duration::days(30 * interval))
                }
                _ => Some(now + chrono::Duration::days(1)),
            }
        }
        TaskSchedule::BusinessDay { day_spec, holidays } => {
            // Advance to the next occurrence strictly after today
            let tomorrow = (Utc::now() + chrono::Duration::days(1)).date_naive();
            let cal = load_calendar_for_country(holidays).unwrap_or_default();
            day_spec
                .next_occurrence(tomorrow, &cal)
                .map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc())
        }
        _ => None,
    }
}

fn extract_rrule_field(rrule: &str, field: &str) -> Option<String> {
    let prefix = format!("{field}=");
    for part in rrule.split(';') {
        if let Some(val) = part.strip_prefix(&prefix) {
            return Some(val.to_string());
        }
    }
    None
}

fn parse_urgency(s: &str) -> Urgency {
    match s {
        "Low" => Urgency::Low,
        "High" => Urgency::High,
        "Ignore" => Urgency::Ignore,
        _ => Urgency::Medium,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_core::schedule::TaskSchedule;

    async fn test_db() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = crate::db::open(&db_path).await.unwrap();
        (pool, dir)
    }

    #[tokio::test]
    async fn create_and_get_task() {
        let (pool, _dir) = test_db().await;
        let store = TaskStore::new(pool);

        let task = Task {
            title: "Pay electricity bill".into(),
            notes: Some("EDP €67.42".into()),
            schedule: TaskSchedule::Once {
                due: Utc::now() + chrono::Duration::days(3),
            },
            context: vec!["ref 678901234".into()],
            conditions: vec![],
            urgency: Urgency::High,
        };

        let id = store.create(&task, "cortex").await.unwrap();
        let stored = store.get(&id).await.unwrap().unwrap();

        assert_eq!(stored.title, "Pay electricity bill");
        assert_eq!(stored.notes.as_deref(), Some("EDP €67.42"));
        assert_eq!(stored.urgency, Urgency::High);
        assert_eq!(stored.context, vec!["ref 678901234".to_string()]);
        assert!(stored.next_trigger.is_some());
        assert!(stored.completed_at.is_none());
    }

    #[tokio::test]
    async fn list_active_excludes_completed() {
        let (pool, _dir) = test_db().await;
        let store = TaskStore::new(pool);

        let t1 = Task {
            title: "Task A".into(),
            notes: None,
            schedule: TaskSchedule::Once {
                due: Utc::now() + chrono::Duration::days(1),
            },
            context: vec![],
            conditions: vec![],
            urgency: Urgency::Medium,
        };
        let t2 = Task {
            title: "Task B".into(),
            notes: None,
            schedule: TaskSchedule::Once {
                due: Utc::now() + chrono::Duration::days(2),
            },
            context: vec![],
            conditions: vec![],
            urgency: Urgency::Medium,
        };

        let id1 = store.create(&t1, "user").await.unwrap();
        store.create(&t2, "user").await.unwrap();

        assert_eq!(store.list_active().await.unwrap().len(), 2);

        store.complete(&id1).await.unwrap();
        assert_eq!(store.list_active().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn complete_recurring_advances_trigger() {
        let (pool, _dir) = test_db().await;
        let store = TaskStore::new(pool);

        let task = Task {
            title: "Water plants".into(),
            notes: None,
            schedule: TaskSchedule::Recurring {
                rrule: "FREQ=WEEKLY;INTERVAL=1".into(),
            },
            context: vec![],
            conditions: vec![],
            urgency: Urgency::Low,
        };

        let id = store.create(&task, "user").await.unwrap();
        let before = store.get(&id).await.unwrap().unwrap();
        let before_trigger = before.next_trigger.unwrap();

        store.complete(&id).await.unwrap();
        let after = store.get(&id).await.unwrap().unwrap();

        // Should NOT be completed (recurring), should have advanced trigger
        assert!(after.completed_at.is_none());
        assert!(after.next_trigger.unwrap() > before_trigger);
    }

    #[tokio::test]
    async fn update_task_fields() {
        let (pool, _dir) = test_db().await;
        let store = TaskStore::new(pool);

        let task = Task {
            title: "Old title".into(),
            notes: None,
            schedule: TaskSchedule::Once {
                due: Utc::now() + chrono::Duration::days(1),
            },
            context: vec![],
            conditions: vec![],
            urgency: Urgency::Low,
        };

        let id = store.create(&task, "user").await.unwrap();

        let patch = TaskPatch {
            title: Some("New title".into()),
            notes: Some("Added notes".into()),
            schedule: None,
            urgency: Some(Urgency::High),
        };

        store.update(&id, &patch).await.unwrap();
        let updated = store.get(&id).await.unwrap().unwrap();
        assert_eq!(updated.title, "New title");
        assert_eq!(updated.notes.as_deref(), Some("Added notes"));
        assert_eq!(updated.urgency, Urgency::High);
    }

    #[tokio::test]
    async fn list_due_finds_overdue_tasks() {
        let (pool, _dir) = test_db().await;
        let store = TaskStore::new(pool);

        // Task due in the past
        let past = Task {
            title: "Overdue task".into(),
            notes: None,
            schedule: TaskSchedule::Once {
                due: Utc::now() - chrono::Duration::hours(1),
            },
            context: vec![],
            conditions: vec![],
            urgency: Urgency::High,
        };
        // Task due in future
        let future = Task {
            title: "Future task".into(),
            notes: None,
            schedule: TaskSchedule::Once {
                due: Utc::now() + chrono::Duration::days(5),
            },
            context: vec![],
            conditions: vec![],
            urgency: Urgency::Low,
        };

        store.create(&past, "user").await.unwrap();
        store.create(&future, "user").await.unwrap();

        let due = store.list_due().await.unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].title, "Overdue task");
    }

    #[test]
    fn schedule_serialization_roundtrip() {
        let schedules = vec![
            TaskSchedule::Once { due: Utc::now() },
            TaskSchedule::Recurring {
                rrule: "FREQ=DAILY;INTERVAL=3".into(),
            },
        ];
        for schedule in schedules {
            let (stype, sdata) = serialize_schedule(&schedule).unwrap();
            let parsed = deserialize_schedule(&stype, &sdata).unwrap();
            // Compare discriminant
            assert_eq!(
                std::mem::discriminant(&schedule),
                std::mem::discriminant(&parsed)
            );
        }
    }

    #[test]
    fn rrule_field_extraction() {
        assert_eq!(
            extract_rrule_field("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO", "FREQ"),
            Some("WEEKLY".into())
        );
        assert_eq!(
            extract_rrule_field("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO", "INTERVAL"),
            Some("2".into())
        );
        assert_eq!(
            extract_rrule_field("FREQ=DAILY", "INTERVAL"),
            None
        );
    }
}
