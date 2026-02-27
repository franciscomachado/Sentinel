/// Cortex operating mode — tracks API availability and auto-degrades.
///
/// When the Anthropic API is unreachable, Sentinel falls back to local-only
/// intelligence. Calendar, tasks, departure, sports, and weather still work.
/// Only AI-dependent features (briefing reasoning, email triage, Signal queries)
/// are affected.

use std::sync::atomic::{AtomicU8, Ordering};

/// The three operating modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CortexMode {
    /// All features available — AI fully functional.
    Full = 0,
    /// AI unavailable — local-only notifications, no reasoning.
    Degraded = 1,
    /// Explicitly offline (user-initiated or extended outage).
    Offline = 2,
}

impl CortexMode {
    fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Full,
            1 => Self::Degraded,
            2 => Self::Offline,
            _ => Self::Degraded,
        }
    }
}

impl std::fmt::Display for CortexMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Full => write!(f, "full"),
            Self::Degraded => write!(f, "degraded"),
            Self::Offline => write!(f, "offline"),
        }
    }
}

/// Thread-safe mode tracker with automatic recovery.
pub struct ModeTracker {
    mode: AtomicU8,
    /// Consecutive API failure count.
    failures: AtomicU8,
    /// Threshold of consecutive failures before switching to degraded.
    failure_threshold: u8,
}

impl ModeTracker {
    pub fn new() -> Self {
        Self {
            mode: AtomicU8::new(CortexMode::Full as u8),
            failures: AtomicU8::new(0),
            failure_threshold: 3,
        }
    }

    /// Current operating mode.
    pub fn mode(&self) -> CortexMode {
        CortexMode::from_u8(self.mode.load(Ordering::Relaxed))
    }

    /// Record a successful API call — resets failure count and restores Full mode.
    pub fn record_success(&self) -> bool {
        let was_degraded = self.mode() != CortexMode::Full;
        self.failures.store(0, Ordering::Relaxed);
        self.mode.store(CortexMode::Full as u8, Ordering::Relaxed);
        was_degraded // true if we just recovered
    }

    /// Record an API failure. Returns true if this failure caused a mode transition
    /// to Degraded.
    pub fn record_failure(&self) -> bool {
        let prev = self.failures.fetch_add(1, Ordering::Relaxed);
        if prev + 1 >= self.failure_threshold && self.mode() == CortexMode::Full {
            self.mode.store(CortexMode::Degraded as u8, Ordering::Relaxed);
            return true; // Just transitioned
        }
        false
    }

    /// Force a specific mode (e.g., user sets offline).
    pub fn set_mode(&self, mode: CortexMode) {
        self.mode.store(mode as u8, Ordering::Relaxed);
    }

    /// Whether AI features are available.
    pub fn ai_available(&self) -> bool {
        self.mode() == CortexMode::Full
    }
}

impl Default for ModeTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Format a degraded-mode fallback notification for a trigger.
pub fn degraded_fallback(trigger_desc: &str) -> String {
    format!(
        "⚠️ Running in local mode — AI unavailable.\n\
         {trigger_desc}\n\
         Basic notifications only. Will recover automatically when the API is back."
    )
}

/// Format a reduced morning briefing (no AI reasoning).
pub fn degraded_briefing(calendar: &str, tasks: &str, weather: &str) -> String {
    let mut parts = Vec::new();
    if !calendar.is_empty() {
        parts.push(format!("📅 Calendar:\n{calendar}"));
    }
    if !tasks.is_empty() {
        parts.push(format!("✅ Tasks:\n{tasks}"));
    }
    if !weather.is_empty() {
        parts.push(format!("🌤️ Weather:\n{weather}"));
    }
    if parts.is_empty() {
        return "⚠️ Running in local mode — no data available.".into();
    }
    format!("⚠️ Local mode briefing (AI unavailable):\n\n{}", parts.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_in_full_mode() {
        let tracker = ModeTracker::new();
        assert_eq!(tracker.mode(), CortexMode::Full);
        assert!(tracker.ai_available());
    }

    #[test]
    fn degrades_after_threshold() {
        let tracker = ModeTracker::new();

        // First two failures don't degrade
        assert!(!tracker.record_failure());
        assert_eq!(tracker.mode(), CortexMode::Full);
        assert!(!tracker.record_failure());
        assert_eq!(tracker.mode(), CortexMode::Full);

        // Third failure triggers degradation
        assert!(tracker.record_failure());
        assert_eq!(tracker.mode(), CortexMode::Degraded);
        assert!(!tracker.ai_available());
    }

    #[test]
    fn recovers_on_success() {
        let tracker = ModeTracker::new();

        // Degrade
        tracker.record_failure();
        tracker.record_failure();
        tracker.record_failure();
        assert_eq!(tracker.mode(), CortexMode::Degraded);

        // Recover
        let was_degraded = tracker.record_success();
        assert!(was_degraded);
        assert_eq!(tracker.mode(), CortexMode::Full);
        assert!(tracker.ai_available());
    }

    #[test]
    fn degraded_briefing_format() {
        let result = degraded_briefing(
            "09:00 Dentist",
            "Buy groceries",
            "14°C, cloudy",
        );
        assert!(result.contains("Local mode"));
        assert!(result.contains("Dentist"));
        assert!(result.contains("Buy groceries"));
        assert!(result.contains("14°C"));
    }
}
