use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::{Urgency, WatcherId};

/// Central event type emitted by all watchers into the event bus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WatchEvent {
    Email(EmailEvent),
    Calendar(CalendarChange),
    Task(TaskEvent),
    Departure(DepartureEvent),
    Signal(SignalMessage),
    Weather(WeatherUpdate),
    Schedule(ScheduledTrigger),
    Sports(SportsAlert),
    Cultural(CulturalAlert),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailEvent {
    pub id: crate::capability::EmailId,
    pub from: String,
    pub to: Vec<String>,
    pub subject: String,
    pub preview: String,
    pub timestamp: DateTime<Utc>,
    pub is_reply: bool,
    pub has_attachments: bool,
    pub urgency: Urgency,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CalendarChange {
    Created(crate::capability::CalendarEvent),
    Modified(crate::capability::CalendarEvent),
    Deleted(crate::capability::EventId),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskEvent {
    pub task_id: crate::capability::TaskId,
    pub kind: TaskEventKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskEventKind {
    Due,
    Overdue,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepartureEvent {
    pub destination: String,
    pub event_time: DateTime<Utc>,
    pub travel_minutes: u32,
    pub leave_by: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalMessage {
    /// Phone number of the sender (e.g. "+351969696969").
    pub sender: String,
    pub text: String,
    pub timestamp: DateTime<Utc>,
    pub attachments: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeatherUpdate {
    pub location: String,
    pub temperature_c: f64,
    pub conditions: String,
    pub forecast: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ScheduledTrigger {
    MorningReflection,
    MorningBriefing,
    WeeklyReflection,
    WeeklyPlanning,
    MonthlyReflection,
    DepartureCheck,
    RhythmEngineRun,
}

/// Context provided to watchers.
pub struct WatcherContext {
    pub watcher_id: WatcherId,
    pub event_tx: tokio::sync::mpsc::Sender<WatchEvent>,
}

/// A cultural event surfaced by the taste-scoring engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CulturalAlert {
    pub title: String,
    pub venue: Option<String>,
    pub date: Option<DateTime<Utc>>,
    pub source_name: String,
    pub match_score: f64,
}

/// A sports session that's coming up or currently live.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SportsAlert {
    pub series_id: String,
    pub series_name: String,
    pub round_name: String,
    pub session_name: String,
    /// Session start in UTC.
    pub start_utc: DateTime<Utc>,
    /// Whether this is a spoiler-protected series.
    pub spoiler_protect: bool,
}

/// A sports session resolved to the user's timezone, used by the state compiler.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SportsEvent {
    pub series_id: String,
    pub series_name: String,
    pub round_name: String,
    pub session_name: String,
    pub start_utc: DateTime<Utc>,
    /// User-local time string (e.g. "14:00").
    pub local_time: String,
    /// User-local date string (e.g. "2026-03-15").
    pub local_date: String,
    pub spoiler_protect: bool,
}
