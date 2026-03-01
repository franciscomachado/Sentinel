use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::{Dish, MealEntry, Urgency};

/// All possible actions Sentinel can take. Exhaustive. No wildcards.
///
/// Rust enums have no `Other(String)` variant. If the LLM's response doesn't
/// deserialize into one of these variants, it's dropped. Prompt injection fails
/// at the JSON parsing step — there's no variant to put it in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Capability {
    // === READ (auto-approved) ===
    EmailRead(EmailQuery),
    CalendarRead(CalendarQuery),
    TaskListRead,
    WeatherFetch(Location),
    RoutingQuery(RouteRequest),

    // === WRITE (require human approval) ===
    CalendarEventCreate(CalendarEvent),
    CalendarEventModify(EventId, CalendarEventPatch),
    CalendarEventDelete(EventId),
    TaskCreate(Task),
    TaskComplete(TaskId),
    TaskModify(TaskId, TaskPatch),
    ReminderSet(Reminder),
    BringAdd(BringItem),
    BringRemove(BringItem),
    EmailDraft(EmailDraft),
    EmailReply(EmailId, DraftReply),
    SignalReply(String),

    // === HOUSEHOLD (require human approval) ===
    /// Add a dish to the household recipe catalog.
    DishAdd(Dish),
    /// Write a confirmed set of meal-plan entries (replaces any existing entries for those dates).
    MealPlanSet(Vec<MealEntry>),

    // === NEVER (don't exist, can't exist) ===
    // ExecuteCommand    — NOT IN THE ENUM
    // FileWrite         — NOT IN THE ENUM
    // FileRead          — NOT IN THE ENUM
    // EmailSend         — NOT IN THE ENUM (only Draft)
    // NetworkRequest    — NOT IN THE ENUM
    // InstallPackage    — NOT IN THE ENUM
    // CredentialAccess  — NOT IN THE ENUM
    // CrossUserAccess   — NOT IN THE ENUM
}

impl Capability {
    /// Returns the kind discriminant for this capability (for audit/policy).
    pub fn kind(&self) -> CapabilityKind {
        match self {
            Self::EmailRead(_) => CapabilityKind::EmailRead,
            Self::CalendarRead(_) => CapabilityKind::CalendarRead,
            Self::TaskListRead => CapabilityKind::TaskListRead,
            Self::WeatherFetch(_) => CapabilityKind::WeatherFetch,
            Self::RoutingQuery(_) => CapabilityKind::RoutingQuery,
            Self::CalendarEventCreate(_) => CapabilityKind::CalendarEventCreate,
            Self::CalendarEventModify(_, _) => CapabilityKind::CalendarEventModify,
            Self::CalendarEventDelete(_) => CapabilityKind::CalendarEventDelete,
            Self::TaskCreate(_) => CapabilityKind::TaskCreate,
            Self::TaskComplete(_) => CapabilityKind::TaskComplete,
            Self::TaskModify(_, _) => CapabilityKind::TaskModify,
            Self::ReminderSet(_) => CapabilityKind::ReminderSet,
            Self::BringAdd(_) => CapabilityKind::BringAdd,
            Self::BringRemove(_) => CapabilityKind::BringRemove,
            Self::EmailDraft(_) => CapabilityKind::EmailDraft,
            Self::EmailReply(_, _) => CapabilityKind::EmailReply,
            Self::SignalReply(_) => CapabilityKind::SignalReply,
            Self::DishAdd(_) => CapabilityKind::DishAdd,
            Self::MealPlanSet(_) => CapabilityKind::MealPlanSet,
        }
    }

    /// Whether this is a read-only capability (safe to auto-approve).
    pub fn is_read(&self) -> bool {
        matches!(
            self,
            Self::EmailRead(_)
                | Self::CalendarRead(_)
                | Self::TaskListRead
                | Self::WeatherFetch(_)
                | Self::RoutingQuery(_)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CapabilityKind {
    EmailRead,
    CalendarRead,
    TaskListRead,
    WeatherFetch,
    RoutingQuery,
    CalendarEventCreate,
    CalendarEventModify,
    CalendarEventDelete,
    TaskCreate,
    TaskComplete,
    TaskModify,
    ReminderSet,
    BringAdd,
    BringRemove,
    EmailDraft,
    EmailReply,
    SignalReply,
    DishAdd,
    MealPlanSet,
}

impl std::fmt::Display for CapabilityKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

// ── Capability parameter types ──────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailQuery {
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarQuery {
    #[serde(default)]
    pub start: Option<DateTime<Utc>>,
    #[serde(default)]
    pub end: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Location {
    pub name: String,
    #[serde(default)]
    pub lat: Option<f64>,
    #[serde(default)]
    pub lon: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Coordinates {
    pub lat: f64,
    pub lon: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteRequest {
    pub origin: Location,
    pub destination: Location,
    #[serde(default)]
    pub departure_time: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarEvent {
    pub title: String,
    pub start: DateTime<Utc>,
    #[serde(default)]
    pub end: Option<DateTime<Utc>>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub all_day: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct EventId(pub String);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarEventPatch {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub start: Option<DateTime<Utc>>,
    #[serde(default)]
    pub end: Option<DateTime<Utc>>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub title: String,
    #[serde(default)]
    pub notes: Option<String>,
    pub schedule: crate::schedule::TaskSchedule,
    #[serde(default)]
    pub context: Vec<String>,
    #[serde(default)]
    pub conditions: Vec<String>,
    #[serde(default = "default_urgency")]
    pub urgency: Urgency,
}

fn default_urgency() -> Urgency {
    Urgency::Medium
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct TaskId(pub String);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPatch {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub schedule: Option<crate::schedule::TaskSchedule>,
    #[serde(default)]
    pub urgency: Option<Urgency>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reminder {
    pub message: String,
    pub time: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BringItem {
    pub name: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub context: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailDraft {
    pub to: Vec<String>,
    pub subject: String,
    pub body: String,
    #[serde(default)]
    pub account: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct EmailId {
    pub account: String,
    pub uid: u32,
}

impl EmailId {
    pub fn new(account: String, uid: u32) -> Self {
        Self { account, uid }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftReply {
    pub body: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_round_trip() {
        let cap = Capability::CalendarEventCreate(CalendarEvent {
            title: "Dentist".into(),
            start: Utc::now(),
            end: None,
            location: Some("Clínica São João".into()),
            description: None,
            all_day: false,
        });
        let json = serde_json::to_string(&cap).unwrap();
        let parsed: Capability = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.kind(), CapabilityKind::CalendarEventCreate);
    }

    #[test]
    fn capability_rejects_unknown_variant() {
        let bad_json = r#"{"ExecuteCommand": "rm -rf /"}"#;
        let result = serde_json::from_str::<Capability>(bad_json);
        assert!(result.is_err());
    }

    #[test]
    fn bring_item_round_trip() {
        let cap = Capability::BringAdd(BringItem {
            name: "Minced meat".into(),
            category: Some("Meat".into()),
            context: Some("for bolognese".into()),
        });
        let json = serde_json::to_string(&cap).unwrap();
        let parsed: Capability = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, Capability::BringAdd(_)));
    }

    #[test]
    fn dish_add_round_trip() {
        use crate::types::Dish;
        let cap = Capability::DishAdd(Dish {
            id: None,
            name: "Arroz de polvo".into(),
            protein: Some("polvo".into()),
            carb: Some("arroz".into()),
            notes: None,
        });
        let json = serde_json::to_string(&cap).unwrap();
        let parsed: Capability = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.kind(), CapabilityKind::DishAdd);
        assert!(!parsed.is_read());
    }

    #[test]
    fn meal_plan_set_round_trip() {
        use crate::types::MealEntry;
        let cap = Capability::MealPlanSet(vec![
            MealEntry {
                date: "2026-03-03".into(),
                meal_type: "dinner".into(),
                description: "Arroz de polvo".into(),
                ingredients: vec![],
                created_by: "sentinel".into(),
            },
        ]);
        let json = serde_json::to_string(&cap).unwrap();
        let parsed: Capability = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.kind(), CapabilityKind::MealPlanSet);
        assert!(!parsed.is_read());
    }

    #[test]
    fn email_id_equality() {
        let a = EmailId::new("personal".into(), 42);
        let b = EmailId::new("personal".into(), 42);
        assert_eq!(a, b);
    }

    // ── Injection-specific capability tests ─────────────────────

    #[test]
    fn rejects_file_write() {
        let json = r#"{"FileWrite": {"path": "/tmp/evil", "content": "malware"}}"#;
        assert!(serde_json::from_str::<Capability>(json).is_err());
    }

    #[test]
    fn rejects_file_read() {
        let json = r#"{"FileRead": "/etc/shadow"}"#;
        assert!(serde_json::from_str::<Capability>(json).is_err());
    }

    #[test]
    fn rejects_network_request() {
        let json = r#"{"NetworkRequest": {"url": "https://evil.com", "method": "POST"}}"#;
        assert!(serde_json::from_str::<Capability>(json).is_err());
    }

    #[test]
    fn rejects_install_package() {
        let json = r#"{"InstallPackage": "cryptominer"}"#;
        assert!(serde_json::from_str::<Capability>(json).is_err());
    }

    #[test]
    fn rejects_credential_access() {
        let json = r#"{"CredentialAccess": "api_key"}"#;
        assert!(serde_json::from_str::<Capability>(json).is_err());
    }

    #[test]
    fn rejects_cross_user_access() {
        let json = r#"{"CrossUserAccess": {"user": "admin", "action": "read"}}"#;
        assert!(serde_json::from_str::<Capability>(json).is_err());
    }

    #[test]
    fn rejects_email_send_not_draft() {
        // EmailDraft exists, EmailSend does not — prevents unsupervised sending
        let json = r#"{"EmailSend": {"to": ["x@x.com"], "subject": "hi", "body": "wire money"}}"#;
        assert!(serde_json::from_str::<Capability>(json).is_err());
    }

    #[test]
    fn rejects_arbitrary_json_object() {
        let json = r#"{"SomethingNew": {"anything": true}}"#;
        assert!(serde_json::from_str::<Capability>(json).is_err());
    }
}
