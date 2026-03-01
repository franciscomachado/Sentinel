use serde::Deserialize;
use std::path::Path;

use crate::error::SentinelError;

/// Top-level Sentinel configuration, loaded from TOML.
#[derive(Debug, Clone, Deserialize)]
pub struct SentinelConfig {
    pub user: UserConfig,
    pub ai: Option<AiConfig>,
    pub email: Option<EmailConfig>,
    pub signal: Option<SignalConfig>,
    pub calendar: Option<CalendarConfig>,
    pub weather: Option<WeatherConfig>,
    pub routing: Option<RoutingConfig>,
    pub departure: Option<DepartureConfig>,
    pub policy: PolicyConfig,
    pub privacy: PrivacyConfig,
    pub integrations: Option<IntegrationsConfig>,
    pub sports: Option<SportsConfig>,
    pub cultural: Option<CulturalConfig>,
    pub household: Option<HouseholdConfig>,
    /// Override the default schedule. When absent, a sensible default schedule is used.
    #[serde(default)]
    pub schedule: Vec<crate::schedule::ScheduleEntry>,
}

/// AI provider configuration.
///
/// Sentinel is designed and tuned for Claude (Anthropic). Other providers
/// are supported but results may vary.
///
/// Anthropic uses its own API format. All other providers (OpenAI, DeepSeek,
/// Gemini, Groq, Mistral, Together, etc.) use the OpenAI-compatible
/// `/v1/chat/completions` endpoint — set the provider name, model, and
/// `api_base` and it should just work.
#[derive(Debug, Clone, Deserialize)]
pub struct AiConfig {
    /// Provider name. Any string is accepted:
    /// - "anthropic" (default) — uses the Anthropic Messages API
    /// - "ollama" — local Ollama, no API key required
    /// - anything else ("openai", "deepseek", "gemini", "groq", "mistral",
    ///   "together", ...) — uses the OpenAI-compatible chat completions API
    #[serde(default = "default_ai_provider")]
    pub provider: String,
    /// Model name. Required for non-standard providers; optional for
    /// anthropic (defaults to claude-sonnet-4-20250514) and ollama (llama3.1).
    pub model: Option<String>,
    /// API base URL. Required for non-standard providers unless they use
    /// the default OpenAI URL. Well-known defaults:
    /// - anthropic: https://api.anthropic.com/v1/messages
    /// - openai: https://api.openai.com/v1/chat/completions
    /// - ollama: http://localhost:11434/v1/chat/completions
    pub api_base: Option<String>,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            provider: default_ai_provider(),
            model: None,
            api_base: None,
        }
    }
}

fn default_ai_provider() -> String { "anthropic".into() }

impl SentinelConfig {
    /// Load configuration from a TOML file path.
    pub fn load(path: &Path) -> Result<Self, SentinelError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| SentinelError::ConfigError(format!("{}: {e}", path.display())))?;
        Self::from_toml(&content)
    }

    /// Parse configuration from a TOML string.
    pub fn from_toml(toml_str: &str) -> Result<Self, SentinelError> {
        toml::from_str(toml_str).map_err(|e| SentinelError::ConfigError(e.to_string()))
    }

    /// Resolve config path: explicit flag, env, or XDG default.
    pub fn resolve_path(explicit: Option<&Path>) -> std::path::PathBuf {
        if let Some(p) = explicit {
            return p.to_owned();
        }
        if let Ok(p) = std::env::var("SENTINEL_CONFIG") {
            return std::path::PathBuf::from(p);
        }
        let dirs = directories::ProjectDirs::from("", "", "sentinel")
            .expect("unable to determine config directory");
        dirs.config_dir().join("sentinel.toml")
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct UserConfig {
    pub name: String,
    pub timezone: String,
    pub locale: String,
    /// Customise the assistant's name. Defaults to "Sentinel".
    /// This name is used in system prompts, notifications, and CLI output.
    pub assistant_name: Option<String>,
    /// ISO country code, matches holiday TOML filename (e.g. "PT" → pt.toml).
    pub country: Option<String>,
    /// Region/municipality where you live (matches key in holiday TOML `[regions.<key>]`).
    pub home_region: Option<String>,
    /// Region/municipality where you work, if different from home.
    pub work_region: Option<String>,
}

impl UserConfig {
    /// Returns the configured assistant name, or "Sentinel" if not set.
    pub fn assistant_name(&self) -> &str {
        self.assistant_name.as_deref().unwrap_or("Sentinel")
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct EmailConfig {
    pub accounts: Vec<EmailAccountConfig>,
    pub triage: Option<EmailTriageConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EmailAccountConfig {
    pub name: String,
    pub imap_host: String,
    pub imap_port: u16,
    pub smtp_host: Option<String>,
    pub smtp_port: Option<u16>,
    /// Per-account triage rules. Merged with global `[email.triage]`:
    /// account-level lists are appended to global lists, account-level
    /// `preview_max_chars` overrides the global value.
    pub triage: Option<EmailTriageConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EmailTriageConfig {
    #[serde(default)]
    pub priority_senders: Vec<String>,
    #[serde(default)]
    pub ignore_senders: Vec<String>,
    pub preview_max_chars: Option<usize>,
}

impl EmailTriageConfig {
    /// Merge two triage configs: `self` is the base (global), `overlay` adds on top.
    /// Sender lists are concatenated; `preview_max_chars` from overlay wins if set.
    pub fn merge(&self, overlay: &EmailTriageConfig) -> EmailTriageConfig {
        let mut priority = self.priority_senders.clone();
        priority.extend(overlay.priority_senders.iter().cloned());
        let mut ignore = self.ignore_senders.clone();
        ignore.extend(overlay.ignore_senders.iter().cloned());
        EmailTriageConfig {
            priority_senders: priority,
            ignore_senders: ignore,
            preview_max_chars: overlay.preview_max_chars.or(self.preview_max_chars),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SignalConfig {
    /// Whether the Signal integration is active.
    #[serde(default)]
    pub enabled: bool,
    /// The signal-cli registered phone number (e.g. "+351919191919").
    pub account: String,
    /// Port where signal-cli's HTTP daemon is listening (default: 8083).
    #[serde(default = "default_signal_port")]
    pub port: u16,
    /// Full HTTP JSON-RPC endpoint override. When set, `port` is ignored.
    /// Only needed for non-standard setups (e.g. remote host, custom path).
    pub http_url: Option<String>,
    /// Path to the signal-cli Unix socket for receiving messages.
    /// Default: `$XDG_RUNTIME_DIR/signal-cli/socket`.
    /// signal-cli daemon must be started with `--socket` (or it's the default transport).
    pub socket_path: Option<String>,
    /// Phone numbers allowed to interact with Sentinel.
    /// Messages from unlisted numbers are silently dropped.
    #[serde(default)]
    pub allow_from: Vec<String>,
    /// How to handle group messages: "ignore" (default) or "allowlist".
    #[serde(default = "default_group_policy")]
    pub group_policy: String,
    /// Group IDs allowed when group_policy = "allowlist".
    #[serde(default)]
    pub allowed_groups: Vec<String>,
    /// Whether to send read receipts for processed messages.
    #[serde(default = "default_true")]
    pub send_read_receipts: bool,
}

impl SignalConfig {
    /// The resolved signal-cli JSON-RPC URL (HTTP, for sending).
    /// Uses `http_url` if explicitly set, otherwise constructs from `port`.
    pub fn signal_url(&self) -> String {
        self.http_url.clone().unwrap_or_else(|| {
            format!("http://127.0.0.1:{}/api/v1/rpc", self.port)
        })
    }

    /// The resolved signal-cli Unix socket path (for receiving via subscription).
    /// Uses `socket_path` if set, otherwise `$XDG_RUNTIME_DIR/signal-cli/socket`,
    /// falling back to `/run/signal-cli/socket`.
    pub fn signal_socket(&self) -> String {
        if let Some(ref path) = self.socket_path {
            return path.clone();
        }
        if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
            return format!("{runtime_dir}/signal-cli/socket");
        }
        "/run/signal-cli/socket".to_string()
    }
}

fn default_signal_port() -> u16 {
    8083
}

fn default_group_policy() -> String {
    "ignore".to_string()
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct CalendarConfig {
    pub caldav_url: String,
    /// Poll interval in seconds (default: 300 = 5 minutes).
    #[serde(default = "default_caldav_poll_secs")]
    pub poll_interval_secs: u64,
    /// CalDAV username (if using Basic auth). Can also use SENTINEL_CALDAV_USER env var.
    pub username: Option<String>,
    /// CalDAV password (if using Basic auth). Can also use SENTINEL_CALDAV_PASS env var.
    pub password: Option<String>,
}

fn default_caldav_poll_secs() -> u64 { 300 }

#[derive(Debug, Clone, Deserialize)]
pub struct WeatherConfig {
    /// Home location latitude.
    pub lat: f64,
    /// Home location longitude.
    pub lon: f64,
    /// Poll interval in seconds (default: 3600 = 1 hour).
    #[serde(default = "default_weather_poll_secs")]
    pub poll_interval_secs: u64,
}

fn default_weather_poll_secs() -> u64 { 3600 }

#[derive(Debug, Clone, Deserialize)]
pub struct RoutingConfig {
    /// "osrm" or "tomtom"
    pub provider: String,
    /// Routing API endpoint.
    pub endpoint: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DepartureConfig {
    /// Home coordinates for origin.
    pub home_lat: f64,
    pub home_lon: f64,
    /// Check interval in seconds (default: 900 = 15 minutes).
    #[serde(default = "default_departure_check_secs")]
    pub check_interval_secs: u64,
    /// Hours ahead to scan calendar for events with locations (default: 3).
    #[serde(default = "default_lookahead_hours")]
    pub lookahead_hours: u64,
    /// Extra minutes to add as comfort buffer (default: 5).
    #[serde(default = "default_comfort_buffer")]
    pub comfort_buffer_minutes: u32,
}

fn default_departure_check_secs() -> u64 { 900 }
fn default_lookahead_hours() -> u64 { 3 }
fn default_comfort_buffer() -> u32 { 5 }

#[derive(Debug, Clone, Deserialize)]
pub struct PolicyConfig {
    pub auto_approve_reads: bool,
    pub max_writes_per_hour: u32,
    pub quiet_hours: Option<QuietHoursConfig>,
    pub email: Option<EmailPolicyConfig>,
    pub calendar: Option<CalendarPolicyConfig>,
    pub tasks: Option<TasksPolicyConfig>,
    pub bring: Option<BringPolicyConfig>,
    pub spending: Option<SpendingPolicyConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QuietHoursConfig {
    pub start: String,
    pub end: String,
    pub except: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EmailPolicyConfig {
    pub never_send_to: Vec<String>,
    pub always_confirm_send: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CalendarPolicyConfig {
    pub auto_approve_reminder_creation: bool,
    pub require_confirmation_for_deletion: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TasksPolicyConfig {
    pub auto_approve_completion: bool,
    pub max_recurring_tasks: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BringPolicyConfig {
    pub auto_approve_when_user_requested: bool,
    pub auto_approve_ai_suggested: bool,
    pub notify_partner_on_removal: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SpendingPolicyConfig {
    pub monthly_ai_budget_euros: f64,
    pub warn_at_percentage: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PrivacyConfig {
    pub ledger_retention_days: u32,
    pub audit_retention_days: u32,
    pub email_cache_retention_days: u32,
    pub memory_review_monthly: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IntegrationsConfig {
    pub events: Option<EventsIntegrationConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EventsIntegrationConfig {
    pub provider: String,
    pub sources: Vec<EventSourceConfig>,
    pub max_distance_km: u32,
    pub check_interval_hours: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EventSourceConfig {
    pub name: String,
    pub url: String,
    pub r#type: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SportsConfig {
    #[serde(default)]
    pub motorsport: Vec<SportsSeriesConfig>,
    #[serde(default)]
    pub football: Vec<SportsSeriesConfig>,
    #[serde(default)]
    pub tennis: Vec<SportsSeriesConfig>,
    /// Path to the sports data directory (default: data/sports)
    pub data_dir: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SportsSeriesConfig {
    pub id: String,
    pub name: String,
    /// "follow" (each session), "casual" (weekly mention), "results_only"
    #[serde(default = "default_interest")]
    pub interest: String,
    /// "each_session", "race_only", "weekly_mention"
    #[serde(default = "default_notify")]
    pub notify: String,
    /// Whether to hide results for delayed-viewing series
    #[serde(default)]
    pub spoiler_protect: bool,
}

fn default_interest() -> String { "follow".into() }
fn default_notify() -> String { "each_session".into() }

#[derive(Debug, Clone, Deserialize)]
pub struct CulturalConfig {
    #[serde(default)]
    pub sources: Vec<CulturalSourceConfig>,
    pub taste: Option<TasteProfileConfig>,
    /// How often to check feeds (default: 12 hours)
    #[serde(default = "default_cultural_check_hours")]
    pub check_interval_hours: u32,
    /// Max events to include in briefings (default: 5)
    #[serde(default = "default_cultural_top_n")]
    pub top_n: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CulturalSourceConfig {
    pub name: String,
    /// "feed", "ical", or "local"
    pub r#type: String,
    /// URL for feed/ical sources, file path for local sources
    pub url: Option<String>,
    pub path: Option<String>,
    pub refresh_hours: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TasteProfileConfig {
    #[serde(default)]
    pub likes: Vec<String>,
    #[serde(default)]
    pub maybe: Vec<String>,
    #[serde(default)]
    pub not_interested: Vec<String>,
}

fn default_cultural_check_hours() -> u32 { 12 }
fn default_cultural_top_n() -> usize { 5 }

/// Household configuration — shared surface for multi-user setups.
#[derive(Debug, Clone, Deserialize)]
pub struct HouseholdConfig {
    /// Path to the shared household database.
    pub shared_db_path: String,
    /// Members of the household.
    #[serde(default)]
    pub members: Vec<HouseholdMemberConfig>,
    /// Shopping list provider ("bring" or "local").
    #[serde(default = "default_shopping_provider")]
    pub shopping_provider: String,
    /// Family CalDAV URL (shared calendar).
    pub family_calendar_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HouseholdMemberConfig {
    pub name: String,
    /// This member's sentinel user id.
    pub user_id: String,
}

fn default_shopping_provider() -> String { "bring".into() }
