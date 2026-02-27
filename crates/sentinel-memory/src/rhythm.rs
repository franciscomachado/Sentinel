use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::fmt;

/// Minimum occurrences before we consider something a pattern at all.
const MIN_OCCURRENCES: u32 = 3;

/// Threshold to graduate from "Emerging" to a real rhythm.
const ESTABLISHED_THRESHOLD: u32 = 5;

/// After this many multiples of the typical interval without occurrence, mark Dormant.
const DORMANT_MULTIPLIER: f64 = 3.0;

/// Days before next expected occurrence to start showing "ComingUp".
const COMING_UP_WINDOW_DAYS: f64 = 2.0;

/// Pattern detection over the ledger. Pure math, no AI.
#[derive(Clone)]
pub struct RhythmEngine {
    pool: SqlitePool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rhythm {
    pub activity: String,
    pub typical_interval_secs: u64,
    pub variance_secs: u64,
    pub last_occurrence: DateTime<Utc>,
    pub occurrences: u32,
    pub status: RhythmStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RhythmStatus {
    OnTrack,
    ComingUp { in_days: u32 },
    Overdue { by_days: u32 },
    Dormant,
    Emerging { occurrences: u32 },
}

impl fmt::Display for RhythmStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OnTrack => write!(f, "OnTrack"),
            Self::ComingUp { in_days } => write!(f, "ComingUp({in_days}d)"),
            Self::Overdue { by_days } => write!(f, "Overdue({by_days}d)"),
            Self::Dormant => write!(f, "Dormant"),
            Self::Emerging { occurrences } => write!(f, "Emerging({occurrences})"),
        }
    }
}

impl fmt::Display for Rhythm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let interval_days = self.typical_interval_secs as f64 / 86400.0;
        let variance_days = self.variance_secs as f64 / 86400.0;
        write!(
            f,
            "{}: {} (typical: every {:.0}-{:.0} days)",
            self.activity,
            self.status,
            interval_days - variance_days,
            interval_days + variance_days,
        )
    }
}

impl RhythmEngine {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Scan the ledger, detect patterns, update the rhythms table, and return all active rhythms.
    /// This is the main entry point — runs locally, zero AI cost.
    pub async fn compute(&self) -> anyhow::Result<Vec<Rhythm>> {
        // Step 1: Pull all activities grouped by content from the ledger.
        // We normalize the activity key from the content field.
        let activities = self.scan_activities().await?;

        let now = Utc::now();
        let mut rhythms = Vec::new();

        for (activity, timestamps) in &activities {
            if (timestamps.len() as u32) < MIN_OCCURRENCES {
                continue;
            }

            let rhythm = compute_rhythm(activity, timestamps, now);
            rhythms.push(rhythm);
        }

        // Persist to DB
        self.save_rhythms(&rhythms).await?;

        Ok(rhythms)
    }

    /// Retrieve previously computed rhythms (read-only, no recomputation).
    pub async fn get_all(&self) -> anyhow::Result<Vec<Rhythm>> {
        let rows: Vec<(String, i64, i64, String, i64, String)> = sqlx::query_as(
            "SELECT activity, typical_interval_secs, variance_secs,
                    last_occurrence, occurrences, status
             FROM rhythms ORDER BY activity",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_rhythm).collect()
    }

    /// Retrieve only rhythms with actionable status (Overdue, ComingUp).
    pub async fn get_flagged(&self) -> anyhow::Result<Vec<Rhythm>> {
        let rows: Vec<(String, i64, i64, String, i64, String)> = sqlx::query_as(
            "SELECT activity, typical_interval_secs, variance_secs,
                    last_occurrence, occurrences, status
             FROM rhythms
             WHERE status LIKE 'Overdue%' OR status LIKE 'ComingUp%'
             ORDER BY activity",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_rhythm).collect()
    }

    /// Scan the ledger for distinct activity keys and their timestamps.
    /// Groups by a normalized version of the content field.
    async fn scan_activities(&self) -> anyhow::Result<Vec<(String, Vec<DateTime<Utc>>)>> {
        // We group ledger entries by category + a normalized content key.
        // Categories that are too generic (like Observation) are skipped —
        // rhythms emerge from user actions, not system metadata.
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT category, content, timestamp FROM ledger
             WHERE category NOT IN ('Observation', 'Inference')
             ORDER BY category, content, timestamp ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        // Build activity groups: category + normalized content → timestamps
        let mut groups: std::collections::BTreeMap<String, Vec<DateTime<Utc>>> =
            std::collections::BTreeMap::new();

        for (category, content, timestamp) in rows {
            let ts = DateTime::parse_from_rfc3339(&timestamp)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());

            let key = normalize_activity_key(&category, &content);
            groups.entry(key).or_default().push(ts);
        }

        Ok(groups.into_iter().collect())
    }

    /// Save computed rhythms to the database (upsert by activity).
    async fn save_rhythms(&self, rhythms: &[Rhythm]) -> anyhow::Result<()> {
        for rhythm in rhythms {
            let last_occ = rhythm.last_occurrence.to_rfc3339();
            let status_str = rhythm.status.to_string();

            sqlx::query(
                "INSERT INTO rhythms (activity, typical_interval_secs, variance_secs,
                                      last_occurrence, occurrences, status, computed_at)
                 VALUES (?, ?, ?, ?, ?, ?, datetime('now'))
                 ON CONFLICT(activity) DO UPDATE SET
                    typical_interval_secs = excluded.typical_interval_secs,
                    variance_secs = excluded.variance_secs,
                    last_occurrence = excluded.last_occurrence,
                    occurrences = excluded.occurrences,
                    status = excluded.status,
                    computed_at = excluded.computed_at",
            )
            .bind(&rhythm.activity)
            .bind(rhythm.typical_interval_secs as i64)
            .bind(rhythm.variance_secs as i64)
            .bind(&last_occ)
            .bind(rhythm.occurrences as i64)
            .bind(&status_str)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }
}

/// Compute a single rhythm from a sorted list of timestamps.
fn compute_rhythm(activity: &str, timestamps: &[DateTime<Utc>], now: DateTime<Utc>) -> Rhythm {
    let count = timestamps.len() as u32;
    let last = *timestamps.last().unwrap();

    // Compute intervals between consecutive occurrences
    let intervals: Vec<u64> = timestamps
        .windows(2)
        .map(|w| (w[1] - w[0]).num_seconds().unsigned_abs())
        .collect();

    let (mean_secs, variance_secs) = if intervals.is_empty() {
        (0, 0)
    } else {
        let mean = intervals.iter().sum::<u64>() / intervals.len() as u64;
        // Variance: mean absolute deviation (more robust than std dev for small samples)
        let mad = intervals
            .iter()
            .map(|&i| (i as i64 - mean as i64).unsigned_abs())
            .sum::<u64>()
            / intervals.len() as u64;
        (mean, mad)
    };

    let status = classify_status(count, mean_secs, last, now);

    Rhythm {
        activity: activity.to_string(),
        typical_interval_secs: mean_secs,
        variance_secs,
        last_occurrence: last,
        occurrences: count,
        status,
    }
}

/// Determine the rhythm's current status based on when we expect the next occurrence.
fn classify_status(
    occurrences: u32,
    interval_secs: u64,
    last: DateTime<Utc>,
    now: DateTime<Utc>,
) -> RhythmStatus {
    if occurrences < ESTABLISHED_THRESHOLD {
        return RhythmStatus::Emerging { occurrences };
    }

    if interval_secs == 0 {
        return RhythmStatus::OnTrack;
    }

    let elapsed = (now - last).num_seconds().max(0) as u64;
    let expected_next_secs = interval_secs;

    if elapsed as f64 > expected_next_secs as f64 * DORMANT_MULTIPLIER {
        return RhythmStatus::Dormant;
    }

    if elapsed > expected_next_secs {
        let overdue_secs = elapsed - expected_next_secs;
        let overdue_days = (overdue_secs as f64 / 86400.0).ceil() as u32;
        return RhythmStatus::Overdue {
            by_days: overdue_days.max(1),
        };
    }

    let remaining_secs = expected_next_secs - elapsed;
    let remaining_days = remaining_secs as f64 / 86400.0;

    if remaining_days <= COMING_UP_WINDOW_DAYS {
        RhythmStatus::ComingUp {
            in_days: remaining_days.ceil() as u32,
        }
    } else {
        RhythmStatus::OnTrack
    }
}

/// Normalize a ledger entry into an activity key for rhythm grouping.
/// e.g., "MealCooked" + "Made fish stew for dinner" → "meal_cooked:fish stew"
fn normalize_activity_key(category: &str, content: &str) -> String {
    // For categories that are inherently unique activities, use category alone or
    // with a simplified content key.
    let content_lower = content.to_lowercase();
    match category {
        // Meal entries: extract the core dish from content
        "MealCooked" => {
            let dish = extract_dish(&content_lower);
            format!("meal:{dish}")
        }
        // Shopping: just the category
        "ShoppingTrip" => "shopping".to_string(),
        // Tasks: use full content as the key (each task is unique)
        "TaskCompleted" | "TaskSkipped" => {
            format!("task:{content_lower}")
        }
        // Email: group by sender
        "EmailReceived" | "EmailActedOn" => {
            let sender = extract_email_sender(&content_lower);
            format!("email:{sender}")
        }
        // Appointments
        "AppointmentKept" | "AppointmentMissed" => {
            format!("appointment:{content_lower}")
        }
        // User-generated entries: use content directly
        "UserNote" => format!("note:{content_lower}"),
        "UserTracking" => format!("tracking:{content_lower}"),
        "UserInterest" => format!("interest:{content_lower}"),
        // Everything else: category + content
        _ => format!("{}:{content_lower}", category.to_lowercase()),
    }
}

/// Extract a dish name from meal content like "Made fish stew for dinner".
fn extract_dish(content: &str) -> String {
    let stripped = content
        .trim_start_matches("made ")
        .trim_start_matches("cooked ")
        .trim_start_matches("prepared ");
    // Remove trailing "for dinner/lunch/breakfast"
    let stripped = stripped
        .trim_end_matches(" for dinner")
        .trim_end_matches(" for lunch")
        .trim_end_matches(" for breakfast");
    stripped.trim().to_string()
}

/// Extract sender from email content like "From: ana@example.com — Subject".
fn extract_email_sender(content: &str) -> String {
    if let Some(rest) = content.strip_prefix("from: ") {
        if let Some(sender) = rest.split([' ', '—', '–', '-']).next() {
            return sender.trim().to_string();
        }
    }
    content.to_string()
}

fn row_to_rhythm(
    row: (String, i64, i64, String, i64, String),
) -> anyhow::Result<Rhythm> {
    let (activity, interval, variance, last_occ, occurrences, status_str) = row;
    let last = DateTime::parse_from_rfc3339(&last_occ)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let status = parse_status(&status_str);

    Ok(Rhythm {
        activity,
        typical_interval_secs: interval as u64,
        variance_secs: variance as u64,
        last_occurrence: last,
        occurrences: occurrences as u32,
        status,
    })
}

fn parse_status(s: &str) -> RhythmStatus {
    if s == "OnTrack" {
        return RhythmStatus::OnTrack;
    }
    if s == "Dormant" {
        return RhythmStatus::Dormant;
    }
    if let Some(rest) = s.strip_prefix("Overdue(") {
        if let Some(days_str) = rest.strip_suffix("d)") {
            if let Ok(days) = days_str.parse() {
                return RhythmStatus::Overdue { by_days: days };
            }
        }
    }
    if let Some(rest) = s.strip_prefix("ComingUp(") {
        if let Some(days_str) = rest.strip_suffix("d)") {
            if let Ok(days) = days_str.parse() {
                return RhythmStatus::ComingUp { in_days: days };
            }
        }
    }
    if let Some(rest) = s.strip_prefix("Emerging(") {
        if let Some(n_str) = rest.strip_suffix(')') {
            if let Ok(n) = n_str.parse() {
                return RhythmStatus::Emerging { occurrences: n };
            }
        }
    }
    RhythmStatus::OnTrack
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::{Ledger, LedgerCategory, LedgerEntry, LedgerId, LedgerSource};

    async fn test_db() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = crate::db::open(&db_path).await.unwrap();
        (pool, dir)
    }

    /// Insert a ledger entry at a specific timestamp (for deterministic tests).
    async fn insert_at(ledger: &Ledger, pool: &SqlitePool, category: LedgerCategory, content: &str, ts: DateTime<Utc>) {
        let entry = LedgerEntry {
            id: LedgerId(uuid::Uuid::new_v4()),
            timestamp: ts,
            category,
            content: content.to_string(),
            tags: vec![],
            source: LedgerSource::User,
        };
        let id = entry.id.0.to_string();
        let timestamp = entry.timestamp.to_rfc3339();
        let cat = entry.category.to_string();
        let tags = "[]";
        let source = entry.source.to_string();
        sqlx::query(
            "INSERT INTO ledger (id, timestamp, category, content, tags, source)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&timestamp)
        .bind(&cat)
        .bind(content)
        .bind(tags)
        .bind(&source)
        .execute(pool)
        .await
        .unwrap();
    }

    #[test]
    fn normalize_meal_key() {
        assert_eq!(
            normalize_activity_key("MealCooked", "Made fish stew for dinner"),
            "meal:fish stew"
        );
        assert_eq!(
            normalize_activity_key("MealCooked", "Cooked pasta"),
            "meal:pasta"
        );
    }

    #[test]
    fn normalize_email_key() {
        assert_eq!(
            normalize_activity_key("EmailReceived", "From: ana@example.com — Dinner?"),
            "email:ana@example.com"
        );
    }

    #[test]
    fn normalize_shopping_key() {
        assert_eq!(
            normalize_activity_key("ShoppingTrip", "Lidl groceries"),
            "shopping"
        );
    }

    #[test]
    fn status_display_roundtrip() {
        let cases = vec![
            RhythmStatus::OnTrack,
            RhythmStatus::ComingUp { in_days: 2 },
            RhythmStatus::Overdue { by_days: 3 },
            RhythmStatus::Dormant,
            RhythmStatus::Emerging { occurrences: 4 },
        ];
        for status in cases {
            let s = status.to_string();
            let parsed = parse_status(&s);
            assert_eq!(parsed, status, "roundtrip failed for {s}");
        }
    }

    #[test]
    fn classify_emerging() {
        let now = Utc::now();
        let status = classify_status(3, 86400 * 5, now, now);
        assert_eq!(status, RhythmStatus::Emerging { occurrences: 3 });
    }

    #[test]
    fn classify_on_track() {
        let now = Utc::now();
        let last = now - chrono::Duration::days(2);
        // Interval 7 days, last was 2 days ago → 5 days remaining → OnTrack
        let status = classify_status(5, 86400 * 7, last, now);
        assert_eq!(status, RhythmStatus::OnTrack);
    }

    #[test]
    fn classify_coming_up() {
        let now = Utc::now();
        let last = now - chrono::Duration::days(6);
        // Interval 7 days, last was 6 days ago → 1 day remaining → ComingUp
        let status = classify_status(5, 86400 * 7, last, now);
        assert_eq!(status, RhythmStatus::ComingUp { in_days: 1 });
    }

    #[test]
    fn classify_overdue() {
        let now = Utc::now();
        let last = now - chrono::Duration::days(10);
        // Interval 7 days, last was 10 days ago → 3 days overdue
        let status = classify_status(5, 86400 * 7, last, now);
        assert_eq!(status, RhythmStatus::Overdue { by_days: 3 });
    }

    #[test]
    fn classify_dormant() {
        let now = Utc::now();
        let last = now - chrono::Duration::days(30);
        // Interval 7 days, last was 30 days ago → 30/7 > 3x → Dormant
        let status = classify_status(5, 86400 * 7, last, now);
        assert_eq!(status, RhythmStatus::Dormant);
    }

    #[test]
    fn compute_rhythm_intervals() {
        let now = Utc::now();
        let timestamps: Vec<DateTime<Utc>> = (0..6)
            .map(|i| now - chrono::Duration::days(5 * (5 - i)))
            .collect();
        // 6 entries, 5-day interval each
        let rhythm = compute_rhythm("test", &timestamps, now);
        assert_eq!(rhythm.occurrences, 6);
        // ~5 days = 432000 seconds
        let interval_days = rhythm.typical_interval_secs as f64 / 86400.0;
        assert!((interval_days - 5.0).abs() < 0.1, "expected ~5 days, got {interval_days}");
        assert_eq!(rhythm.variance_secs, 0); // perfectly regular
    }

    #[tokio::test]
    async fn full_compute_from_ledger() {
        let (pool, _dir) = test_db().await;
        let ledger = Ledger::new(pool.clone());
        let engine = RhythmEngine::new(pool.clone());

        let now = Utc::now();

        // Insert 5 "fish stew" meals at ~12 day intervals
        for i in 0..5 {
            let ts = now - chrono::Duration::days(12 * (4 - i));
            insert_at(&ledger, &pool, LedgerCategory::MealCooked, "Made fish stew for dinner", ts).await;
        }

        // Insert 3 shopping trips (below established threshold)
        for i in 0..3 {
            let ts = now - chrono::Duration::days(6 * (2 - i));
            insert_at(&ledger, &pool, LedgerCategory::ShoppingTrip, "Lidl", ts).await;
        }

        let rhythms = engine.compute().await.unwrap();

        // Should have 2 rhythms: fish stew (established) + shopping (emerging)
        assert_eq!(rhythms.len(), 2);

        let stew = rhythms.iter().find(|r| r.activity.contains("fish stew")).unwrap();
        assert_eq!(stew.occurrences, 5);
        let interval_days = stew.typical_interval_secs as f64 / 86400.0;
        assert!((interval_days - 12.0).abs() < 0.1, "expected ~12 days, got {interval_days}");

        let shopping = rhythms.iter().find(|r| r.activity == "shopping").unwrap();
        assert_eq!(shopping.occurrences, 3);
        assert!(matches!(shopping.status, RhythmStatus::Emerging { .. }));

        // Verify persistence
        let loaded = engine.get_all().await.unwrap();
        assert_eq!(loaded.len(), 2);

        // Verify flagged query
        let flagged = engine.get_flagged().await.unwrap();
        // The stew rhythm: last occurrence at `now`, interval 12 days → OnTrack, not flagged
        // Shopping: Emerging, not flagged either
        // So flagged should be empty unless one is Overdue/ComingUp
        for r in &flagged {
            assert!(
                matches!(r.status, RhythmStatus::Overdue { .. } | RhythmStatus::ComingUp { .. }),
                "flagged should only contain Overdue/ComingUp, got {:?}",
                r.status
            );
        }
    }
}
