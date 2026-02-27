use std::collections::HashSet;
use std::time::Duration;

use sentinel_core::capability::TaskId;
use sentinel_core::events::{TaskEvent, TaskEventKind, WatchEvent};
use sentinel_memory::tasks::TaskStore;
use tokio::sync::mpsc::Sender;

/// Watches the task store for due/overdue tasks and emits TaskEvents.
pub struct TaskWatcher {
    store: TaskStore,
    poll_interval: Duration,
}

impl TaskWatcher {
    pub fn new(store: TaskStore) -> Self {
        Self {
            store,
            poll_interval: Duration::from_secs(60),
        }
    }

    /// Run the task watcher loop, emitting events on the channel.
    pub async fn run(self, tx: Sender<WatchEvent>) -> anyhow::Result<()> {
        let mut alerted: HashSet<String> = HashSet::new();

        loop {
            match self.store.list_due().await {
                Ok(tasks) => {
                    for task in &tasks {
                        if alerted.contains(&task.id) {
                            continue;
                        }

                        let kind = if let Some(trigger) = task.next_trigger {
                            let overdue_threshold =
                                chrono::Utc::now() - chrono::Duration::minutes(30);
                            if trigger < overdue_threshold {
                                TaskEventKind::Overdue
                            } else {
                                TaskEventKind::Due
                            }
                        } else {
                            TaskEventKind::Due
                        };

                        let event = WatchEvent::Task(TaskEvent {
                            task_id: TaskId(task.id.clone()),
                            kind,
                        });

                        if tx.send(event).await.is_err() {
                            return Ok(()); // channel closed
                        }

                        alerted.insert(task.id.clone());
                    }

                    // Clean up alerted set: remove tasks no longer due
                    let due_ids: HashSet<String> =
                        tasks.iter().map(|t| t.id.clone()).collect();
                    alerted.retain(|id| due_ids.contains(id));
                }
                Err(e) => {
                    tracing::error!(error = %e, "task watcher: failed to query due tasks");
                }
            }

            tokio::time::sleep(self.poll_interval).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_core::capability::Task;
    use sentinel_core::schedule::TaskSchedule;
    use sentinel_core::types::Urgency;

    async fn test_db() -> (sqlx::SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = sentinel_memory::db::open(&db_path).await.unwrap();
        (pool, dir)
    }

    #[tokio::test]
    async fn emits_due_task_event() {
        let (pool, _dir) = test_db().await;
        let store = TaskStore::new(pool);

        // Create a task due in the past
        let task = Task {
            title: "Past due task".into(),
            notes: None,
            schedule: TaskSchedule::Once {
                due: chrono::Utc::now() - chrono::Duration::minutes(5),
            },
            context: vec![],
            conditions: vec![],
            urgency: Urgency::High,
        };
        store.create(&task, "test").await.unwrap();

        let watcher = TaskWatcher::new(store);
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);

        // Run watcher in background, collect first event
        tokio::spawn(async move { watcher.run(tx).await });

        let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();

        match event {
            WatchEvent::Task(te) => {
                // Verify it's a task event with Due or Overdue kind
                assert!(
                    matches!(te.kind, TaskEventKind::Due | TaskEventKind::Overdue)
                );
                assert!(!te.task_id.0.is_empty());
            }
            _ => panic!("expected TaskEvent"),
        }
    }

    #[tokio::test]
    async fn does_not_duplicate_alerts() {
        let (pool, _dir) = test_db().await;
        let store = TaskStore::new(pool);

        let task = Task {
            title: "Only alert once".into(),
            notes: None,
            schedule: TaskSchedule::Once {
                due: chrono::Utc::now() - chrono::Duration::minutes(1),
            },
            context: vec![],
            conditions: vec![],
            urgency: Urgency::Medium,
        };
        store.create(&task, "test").await.unwrap();

        let watcher = TaskWatcher::new(store);
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);

        tokio::spawn(async move { watcher.run(tx).await });

        // Should get exactly one event for this task
        let _first = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();

        // Second recv should timeout (no duplicate)
        let result = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await;
        assert!(result.is_err(), "should not receive duplicate alert");
    }
}
