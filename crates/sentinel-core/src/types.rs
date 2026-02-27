use serde::{Deserialize, Serialize};
use std::fmt;

/// Unique identifier for a watcher instance.
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct WatcherId(pub String);

impl fmt::Display for WatcherId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for WatcherId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

/// Service identifiers for credential lookups.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServiceId {
    Imap(String),
    Smtp(String),
    CalDav,
    Anthropic,
    /// Generic AI provider credential lookup. The string is the provider
    /// name (e.g. "openai", "deepseek", "gemini", "groq").
    Ai(String),
    Routing,
    Bring,
}

impl fmt::Display for ServiceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Imap(host) => write!(f, "imap:{host}"),
            Self::Smtp(host) => write!(f, "smtp:{host}"),
            Self::CalDav => write!(f, "caldav"),
            Self::Anthropic => write!(f, "anthropic"),
            Self::Ai(name) => write!(f, "ai:{name}"),
            Self::Routing => write!(f, "routing"),
            Self::Bring => write!(f, "bring"),
        }
    }
}

/// Urgency levels for triage and notification routing.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Urgency {
    Ignore,
    Low,
    Medium,
    High,
    Urgent,
}

impl Default for Urgency {
    fn default() -> Self {
        Self::Medium
    }
}

impl fmt::Display for Urgency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// Source of an action for audit purposes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ActionSource {
    Watcher(WatcherId),
    Cortex,
    UserDirect,
    Schedule,
}

/// Decision recorded in the audit log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    AutoApproved,
    HumanApproved,
    HumanRejected,
    HumanModified,
    PolicyBlocked,
    ParseFailed,
    RateLimited,
    DegradedSkipped,
}

impl fmt::Display for Decision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// Token cost tracking.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenCost {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cached_tokens: u32,
}

impl TokenCost {
    pub fn total_tokens(&self) -> u32 {
        self.input_tokens + self.output_tokens
    }

    /// Estimated cost in EUR (Claude Sonnet 4 pricing as of 2025).
    pub fn estimated_cost_eur(&self) -> f64 {
        let input_cost = (self.input_tokens - self.cached_tokens) as f64 * 3.0 / 1_000_000.0;
        let cached_cost = self.cached_tokens as f64 * 0.3 / 1_000_000.0;
        let output_cost = self.output_tokens as f64 * 15.0 / 1_000_000.0;
        input_cost + cached_cost + output_cost
    }
}

/// Integration categories.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntegrationCategory {
    Email,
    Calendar,
    Tasks,
    Shopping,
    Messaging,
    Routing,
    Weather,
}

/// Credential requirement for integration setup.
#[derive(Debug, Clone)]
pub struct CredentialRequirement {
    pub key: String,
    pub description: String,
    pub secret: bool,
}

/// Travel mode — activated when the user is away from home.
/// Adjusts weather location, departure origin, suspends meal planning,
/// and shifts timezone for briefings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TravelMode {
    pub destination: String,
    pub hotel: Option<String>,
    pub start_date: String,
    pub end_date: String,
    /// Override timezone while travelling (e.g. "Europe/London").
    pub timezone_override: Option<String>,
    /// Override weather location (lat/lon).
    pub weather_lat: Option<f64>,
    pub weather_lon: Option<f64>,
    pub active: bool,
}

/// A weekly meal plan entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MealEntry {
    /// Date as "YYYY-MM-DD".
    pub date: String,
    /// "breakfast", "lunch", "dinner", or "snack".
    pub meal_type: String,
    /// Description of the meal.
    pub description: String,
    /// Ingredients needed.
    #[serde(default)]
    pub ingredients: Vec<String>,
    /// Who planned this meal.
    pub created_by: String,
}

/// A shopping list item with provenance tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShoppingItem {
    pub id: Option<i64>,
    pub item: String,
    pub category: Option<String>,
    pub added_by: String,
    pub context: Option<String>,
    pub purchased: bool,
}

/// A household task with scheduling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HouseholdTask {
    pub id: String,
    pub title: String,
    pub assigned_to: Option<String>,
    /// "daily", "weekly", "monthly", or "once".
    pub schedule_type: String,
    pub schedule_data: String,
    pub next_trigger: Option<String>,
}

impl TravelMode {
    /// Check if travel mode should be active based on current date.
    pub fn is_currently_active(&self) -> bool {
        if !self.active {
            return false;
        }
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        today >= self.start_date && today <= self.end_date
    }

    /// Summary for the state compiler context.
    pub fn summary(&self) -> String {
        let hotel_str = self.hotel.as_deref()
            .map(|h| format!(", staying at {h}"))
            .unwrap_or_default();
        format!(
            "TRAVEL MODE: {} ({}–{}){hotel_str}. Meal planning suspended.",
            self.destination, self.start_date, self.end_date,
        )
    }
}

/// Engagement level — how actively the user is interacting with Sentinel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EngagementLevel {
    /// User actively reading briefings and sending queries.
    Active,
    /// Reduced interaction — lower notification volume.
    Quiet,
    /// No interaction for days — critical alerts only.
    Absent,
}

impl Default for EngagementLevel {
    fn default() -> Self {
        Self::Active
    }
}

impl fmt::Display for EngagementLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Active => write!(f, "active"),
            Self::Quiet => write!(f, "quiet"),
            Self::Absent => write!(f, "absent"),
        }
    }
}
