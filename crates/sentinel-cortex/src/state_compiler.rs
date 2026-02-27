use chrono::{DateTime, Utc};
use sentinel_core::config::{SentinelConfig, SportsConfig, CulturalConfig};
use sentinel_membrane::audit::AuditLog;
use sentinel_memory::ledger::Ledger;
use sentinel_memory::rhythm::RhythmEngine;
use sentinel_memory::state::StateManager;
use sentinel_memory::tasks::TaskStore;

use crate::triage::TriggerType;

/// State compiler: assembles trigger-specific context for LLM calls.
///
/// Compiles the `<current_state>` block from:
/// - Date/time/timezone
/// - User profile (name, locale)
/// - Recent ledger entries (trigger-specific window)
/// - Memories and observations from StateManager
/// - Calendar events, tasks, weather (when watchers exist)
pub struct StateCompiler {
    now: DateTime<Utc>,
    user_name: String,
    timezone: String,
    sections: Vec<String>,
}

impl StateCompiler {
    pub fn new(config: &SentinelConfig) -> Self {
        Self {
            now: Utc::now(),
            user_name: config.user.name.clone(),
            timezone: config.user.timezone.clone(),
            sections: Vec::new(),
        }
    }

    /// Override the current time (useful for testing).
    pub fn at_time(mut self, now: DateTime<Utc>) -> Self {
        self.now = now;
        self
    }

    /// Add a section to the state context.
    pub fn add_section(mut self, header: &str, content: &str) -> Self {
        if !content.is_empty() {
            self.sections.push(format!("## {header}\n{content}"));
        }
        self
    }

    /// Add calendar events.
    pub fn with_calendar(self, events: &str) -> Self {
        self.add_section("Calendar", events)
    }

    /// Add tasks.
    pub fn with_tasks(self, tasks: &str) -> Self {
        self.add_section("Tasks", tasks)
    }

    /// Add weather.
    pub fn with_weather(self, weather: &str) -> Self {
        self.add_section("Weather", weather)
    }

    /// Add memories/observations.
    pub fn with_memories(self, memories: &str) -> Self {
        self.add_section("Memories", memories)
    }

    /// Add travel mode context.
    pub fn with_travel(self, travel: &str) -> Self {
        self.add_section("Travel Mode", travel)
    }

    /// Compile the full state context string (static builder variant).
    pub fn compile(self) -> String {
        self.emit()
    }

    /// Compile trigger-specific context by pulling real data from Ledger and StateManager.
    ///
    /// Different triggers get different context windows:
    /// - Morning briefing: broad context — 24h ledger, all memories, recent observations
    /// - Email triage: recent emails + contact history
    /// - Signal query: broad 72h view + all memories
    /// - Departure: minimal context, just recent entries
    /// - Reflections: wider windows for pattern detection
    pub async fn compile_for_trigger(
        mut self,
        trigger: &TriggerType,
        ledger: &Ledger,
        state: &StateManager,
        rhythm_engine: Option<&RhythmEngine>,
        task_store: Option<&TaskStore>,
        sports_config: Option<&SportsConfig>,
        cultural_config: Option<&CulturalConfig>,
        household: Option<&sentinel_memory::household::HouseholdStore>,
        audit: Option<&AuditLog>,
    ) -> String {
        match trigger {
            TriggerType::MorningBriefing => {
                self = self.pull_ledger_hours(ledger, 24, 20).await;
                self = self.pull_memories(state).await;
                self = self.pull_observations(state, 10).await;
                self = self.pull_rhythms_flagged(rhythm_engine).await;
                self = self.pull_tasks_today(task_store).await;
                self = self.pull_travel_mode(state).await;
                self = self.pull_sports(sports_config).await;
                self = self.pull_cultural(cultural_config).await;
                self = self.pull_household(household).await;
                self = self.pull_engagement(state).await;
            }
            TriggerType::MorningReflection
            | TriggerType::WeeklyReflection
            | TriggerType::MonthlyReflection => {
                // Reflections need wider history for pattern detection
                let (hours, rejection_days) = match trigger {
                    TriggerType::WeeklyReflection => (7 * 24, 7),
                    TriggerType::MonthlyReflection => (30 * 24, 30),
                    _ => (24, 1), // MorningReflection
                };
                self = self.pull_ledger_hours(ledger, hours, 50).await;
                self = self.pull_memories(state).await;
                self = self.pull_observations(state, 20).await;
                self = self.pull_rhythms_all(rhythm_engine).await;
                self = self.pull_rejections(audit, rejection_days).await;
                self = self.pull_engagement(state).await;
            }
            TriggerType::WeeklyPlanning => {
                self = self.pull_ledger_hours(ledger, 7 * 24, 30).await;
                self = self.pull_memories(state).await;
                self = self.pull_observations(state, 10).await;
                self = self.pull_rhythms_all(rhythm_engine).await;
                self = self.pull_tasks_active(task_store).await;
                self = self.pull_travel_mode(state).await;
                self = self.pull_sports(sports_config).await;
                self = self.pull_cultural(cultural_config).await;
                self = self.pull_household(household).await;
            }
            TriggerType::EmailTriage(email) => {
                // Search ledger for history with this sender
                self = self.pull_ledger_search(ledger, &email.from, 10).await;
                self = self.pull_ledger_hours(ledger, 4, 10).await;
                self = self.pull_memories(state).await;
            }
            TriggerType::SignalQuery(_) => {
                // Broad context for free-form queries
                self = self.pull_ledger_hours(ledger, 72, 30).await;
                self = self.pull_memories(state).await;
                self = self.pull_observations(state, 15).await;
                self = self.pull_rhythms_flagged(rhythm_engine).await;
                self = self.pull_tasks_today(task_store).await;
                self = self.pull_travel_mode(state).await;
            }
            TriggerType::UserNote(_) => {
                // Freeform input — broad context to help classify
                self = self.pull_ledger_hours(ledger, 24, 20).await;
                self = self.pull_memories(state).await;
                self = self.pull_rhythms_all(rhythm_engine).await;
            }
            TriggerType::DepartureAlert(_) => {
                self = self.pull_ledger_hours(ledger, 4, 10).await;
                self = self.pull_memories(state).await;
                self = self.pull_observations(state, 5).await;
                self = self.pull_tasks_today(task_store).await;
                self = self.pull_travel_mode(state).await;
            }
            TriggerType::CalendarChange | TriggerType::TaskEvent => {
                self = self.pull_ledger_hours(ledger, 12, 15).await;
                self = self.pull_memories(state).await;
                self = self.pull_tasks_today(task_store).await;
            }
            TriggerType::WeatherUpdate => {
                self = self.pull_ledger_hours(ledger, 4, 5).await;
            }
        }

        self.emit()
    }

    // --- Internal data-pulling helpers ---

    async fn pull_ledger_hours(self, ledger: &Ledger, hours: u32, limit: u32) -> Self {
        match ledger.recent_hours(hours).await {
            Ok(entries) => {
                let entries: Vec<_> = entries.into_iter().take(limit as usize).collect();
                if entries.is_empty() {
                    return self;
                }
                let mut lines = Vec::with_capacity(entries.len());
                for e in &entries {
                    lines.push(format!(
                        "- [{}] {}: {}",
                        e.timestamp.format("%Y-%m-%d %H:%M"),
                        e.category,
                        e.content,
                    ));
                }
                self.add_section("Recent Activity", &lines.join("\n"))
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to pull ledger entries for state");
                self
            }
        }
    }

    async fn pull_ledger_search(self, ledger: &Ledger, query: &str, limit: u32) -> Self {
        match ledger.search(query, limit).await {
            Ok(entries) if !entries.is_empty() => {
                let mut lines = Vec::with_capacity(entries.len());
                for e in &entries {
                    lines.push(format!(
                        "- [{}] {}: {}",
                        e.timestamp.format("%Y-%m-%d %H:%M"),
                        e.category,
                        e.content,
                    ));
                }
                self.add_section("Related History", &lines.join("\n"))
            }
            Ok(_) => self,
            Err(e) => {
                tracing::warn!(error = %e, "failed to search ledger for state");
                self
            }
        }
    }

    async fn pull_memories(self, state: &StateManager) -> Self {
        match state.get_memories().await {
            Ok(memories) if !memories.is_empty() => {
                let mut lines = Vec::with_capacity(memories.len());
                for m in &memories {
                    let tags = if m.tags.is_empty() {
                        String::new()
                    } else {
                        format!(" [{}]", m.tags.join(", "))
                    };
                    lines.push(format!("- {}{tags}", m.content));
                }
                self.add_section("Memories", &lines.join("\n"))
            }
            Ok(_) => self,
            Err(e) => {
                tracing::warn!(error = %e, "failed to pull memories for state");
                self
            }
        }
    }

    async fn pull_observations(self, state: &StateManager, limit: u32) -> Self {
        match state.get_recent_observations(limit).await {
            Ok(obs) if !obs.is_empty() => {
                let mut lines = Vec::with_capacity(obs.len());
                for o in &obs {
                    lines.push(format!(
                        "- {} ({})",
                        o.content,
                        o.created_at.format("%Y-%m-%d"),
                    ));
                }
                self.add_section("Observations", &lines.join("\n"))
            }
            Ok(_) => self,
            Err(e) => {
                tracing::warn!(error = %e, "failed to pull observations for state");
                self
            }
        }
    }

    async fn pull_rhythms_all(self, engine: Option<&RhythmEngine>) -> Self {
        let Some(engine) = engine else { return self };
        match engine.get_all().await {
            Ok(rhythms) if !rhythms.is_empty() => {
                let lines: Vec<String> = rhythms.iter().map(|r| format!("- {r}")).collect();
                self.add_section("Rhythms", &lines.join("\n"))
            }
            Ok(_) => self,
            Err(e) => {
                tracing::warn!(error = %e, "failed to pull rhythms for state");
                self
            }
        }
    }

    async fn pull_rhythms_flagged(self, engine: Option<&RhythmEngine>) -> Self {
        let Some(engine) = engine else { return self };
        match engine.get_flagged().await {
            Ok(rhythms) if !rhythms.is_empty() => {
                let lines: Vec<String> = rhythms.iter().map(|r| format!("- {r}")).collect();
                self.add_section("Rhythm Alerts", &lines.join("\n"))
            }
            Ok(_) => self,
            Err(e) => {
                tracing::warn!(error = %e, "failed to pull flagged rhythms for state");
                self
            }
        }
    }

    async fn pull_tasks_today(self, store: Option<&TaskStore>) -> Self {
        let Some(store) = store else { return self };
        let summary = store.summary_for_today().await;
        if summary.is_empty() {
            return self;
        }
        self.add_section("Tasks Due Today", &summary)
    }

    async fn pull_tasks_active(self, store: Option<&TaskStore>) -> Self {
        let Some(store) = store else { return self };
        let summary = store.summary_active().await;
        if summary.is_empty() {
            return self;
        }
        self.add_section("Active Tasks", &summary)
    }

    async fn pull_travel_mode(self, state: &StateManager) -> Self {
        match state.get_travel_mode().await {
            Ok(Some(mode)) => self.add_section("Travel Mode", &mode.summary()),
            _ => self,
        }
    }

    /// Pull sports data from TOML files.
    /// For morning briefings: today's sessions. For weekly planning: week ahead.
    async fn pull_sports(self, config: Option<&SportsConfig>) -> Self {
        let Some(config) = config else { return self };

        let mut all_series = Vec::new();
        all_series.extend(config.motorsport.iter().cloned());
        all_series.extend(config.football.iter().cloned());
        all_series.extend(config.tennis.iter().cloned());
        if all_series.is_empty() {
            return self;
        }

        let data_dir = config.data_dir.as_deref().unwrap_or("data/sports");
        let watcher = sentinel_watchers::sports::SportsCalendarWatcher::new(
            std::path::PathBuf::from(data_dir),
            all_series,
        );
        let sessions = watcher.load_sessions();

        let tz = chrono::FixedOffset::east_opt(0).unwrap(); // UTC fallback
        let today = sentinel_watchers::sports::today_sessions(&sessions, tz);
        let upcoming = sentinel_watchers::sports::upcoming_sessions(&sessions, 7, tz);

        let mut text = String::new();
        if !today.is_empty() {
            text.push_str(&sentinel_watchers::sports::format_sports_events(&today));
        }
        // Add upcoming sessions not already shown as today
        let extra: Vec<_> = upcoming.into_iter().filter(|u| {
            !today.iter().any(|t| t.series_id == u.series_id && t.session_name == u.session_name && t.start_utc == u.start_utc)
        }).collect();
        if !extra.is_empty() {
            if !text.is_empty() { text.push('\n'); }
            text.push_str("Upcoming:\n");
            text.push_str(&sentinel_watchers::sports::format_sports_events(&extra));
        }

        if text.is_empty() {
            return self;
        }
        self.add_section("Sports", &text)
    }

    /// Pull cultural events by fetching feeds and scoring.
    async fn pull_cultural(self, config: Option<&CulturalConfig>) -> Self {
        let Some(config) = config else { return self };

        let sources: Vec<sentinel_watchers::cultural::EventSource> = config.sources.iter().map(|s| {
            match s.r#type.as_str() {
                "ical" => sentinel_watchers::cultural::EventSource::ICal {
                    name: s.name.clone(),
                    url: s.url.clone().unwrap_or_default(),
                    refresh_hours: s.refresh_hours,
                },
                "local" => sentinel_watchers::cultural::EventSource::LocalFile {
                    name: s.name.clone(),
                    path: std::path::PathBuf::from(s.path.as_deref().unwrap_or("")),
                },
                _ => sentinel_watchers::cultural::EventSource::Feed {
                    name: s.name.clone(),
                    url: s.url.clone().unwrap_or_default(),
                    refresh_hours: s.refresh_hours,
                },
            }
        }).collect();

        let taste = config.taste.as_ref().map(|t| {
            sentinel_watchers::cultural::TasteProfile {
                likes: t.likes.clone(),
                maybe: t.maybe.clone(),
                not_interested: t.not_interested.clone(),
                learned: Vec::new(),
            }
        }).unwrap_or_default();

        let top_n = config.top_n;
        let watcher = sentinel_watchers::cultural::CulturalEventsWatcher::new(
            sources,
            taste,
            config.check_interval_hours,
        );

        let events = watcher.top_events(top_n).await;
        let text = sentinel_watchers::cultural::format_cultural_events(&events);
        if text.is_empty() {
            return self;
        }
        self.add_section("Cultural Events", &text)
    }

    /// Pull household shared surface — meals, shopping list, family events.
    async fn pull_household(self, store: Option<&sentinel_memory::household::HouseholdStore>) -> Self {
        let Some(store) = store else { return self };

        let mut parts = Vec::new();

        let meals = store.format_todays_meals().await;
        if !meals.is_empty() {
            parts.push(format!("Meals today:\n{meals}"));
        }

        let shopping = store.format_shopping_list().await;
        if !shopping.is_empty() {
            parts.push(format!("Shopping list:\n{shopping}"));
        }

        let events = store.format_family_events(7).await;
        if !events.is_empty() {
            parts.push(format!("Family events:\n{events}"));
        }

        if parts.is_empty() {
            return self;
        }
        self.add_section("Household", &parts.join("\n\n"))
    }

    /// Pull recent rejections/modifications from AuditLog for correction learning.
    async fn pull_rejections(self, audit: Option<&AuditLog>, days: u32) -> Self {
        let Some(audit) = audit else { return self };
        match audit.recent_rejections(days).await {
            Ok(rejections) if !rejections.is_empty() => {
                let mut lines = Vec::with_capacity(rejections.len());
                for r in &rejections {
                    lines.push(format!(
                        "- [{}] {} — {} ({})",
                        r.timestamp.format("%Y-%m-%d %H:%M"),
                        r.decision,
                        r.cortex_reasoning,
                        r.capability_kind,
                    ));
                }
                self.add_section("Recent Corrections", &lines.join("\n"))
            }
            Ok(_) => self,
            Err(e) => {
                tracing::warn!(error = %e, "failed to pull rejections for state");
                self
            }
        }
    }

    /// Pull current engagement level for notification volume adjustment.
    async fn pull_engagement(self, state: &StateManager) -> Self {
        match state.engagement_level().await {
            Ok(level) => {
                let note = match level {
                    sentinel_core::types::EngagementLevel::Active => return self, // default, no annotation needed
                    sentinel_core::types::EngagementLevel::Quiet => {
                        "User interaction is reduced. Lower notification volume — only important items."
                    }
                    sentinel_core::types::EngagementLevel::Absent => {
                        "User has been absent for several days. Only critical alerts. Consider a gentle check-in."
                    }
                };
                self.add_section("Engagement", note)
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to pull engagement level for state");
                self
            }
        }
    }

    /// Emit the final compiled string.
    fn emit(self) -> String {
        let weekday = self.now.format("%A").to_string();
        let date = self.now.format("%Y-%m-%d").to_string();
        let time = self.now.format("%H:%M UTC").to_string();

        let mut out = format!(
            "## Context\nUser: {}\nDate: {weekday}, {date}\nTime: {time}\nTimezone: {}\n",
            self.user_name, self.timezone,
        );

        for section in &self.sections {
            out.push('\n');
            out.push_str(section);
            out.push('\n');
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_core::config::*;

    fn test_config() -> SentinelConfig {
        SentinelConfig {
            user: UserConfig {
                name: "John".into(),
                timezone: "Europe/Lisbon".into(),
                locale: "pt-PT".into(),
                country: Some("PT".into()),
                home_region: Some("porto".into()),
                work_region: None,
                assistant_name: None,
            },
            ai: None,
            email: None,
            signal: None,
            calendar: None,
            routing: None,
            weather: None,
            departure: None,
            policy: PolicyConfig {
                auto_approve_reads: true,
                max_writes_per_hour: 20,
                quiet_hours: None,
                email: None,
                calendar: None,
                tasks: None,
                bring: None,
                spending: None,
            },
            privacy: PrivacyConfig {
                ledger_retention_days: 90,
                audit_retention_days: 365,
                email_cache_retention_days: 30,
                memory_review_monthly: true,
            },
            integrations: None,
            sports: None,
            cultural: None,
            household: None,
        }
    }

    #[test]
    fn compiles_basic_state() {
        let config = test_config();
        let state = StateCompiler::new(&config)
            .with_calendar("09:00 Dentist at Clínica São João")
            .with_tasks("Buy groceries (due today)")
            .compile();

        assert!(state.contains("John"));
        assert!(state.contains("Europe/Lisbon"));
        assert!(state.contains("Dentist"));
        assert!(state.contains("Buy groceries"));
    }

    async fn test_db() -> (sqlx::SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = sentinel_memory::db::open(&db_path).await.unwrap();
        (pool, dir)
    }

    #[tokio::test]
    async fn compile_for_trigger_morning_briefing() {
        let (pool, _dir) = test_db().await;
        let ledger = Ledger::new(pool.clone());
        let state_mgr = StateManager::new(pool);

        // Seed some data
        ledger
            .append(&Ledger::entry(
                sentinel_memory::ledger::LedgerCategory::EmailReceived,
                "From: ana@example.com — Dinner tomorrow?".into(),
                sentinel_memory::ledger::LedgerSource::Watcher("email".into()),
            ))
            .await
            .unwrap();

        state_mgr
            .add_memory(
                "Kids don't like fish stew",
                &["food".into(), "preference".into()],
                "weekly_reflection",
            )
            .await
            .unwrap();

        state_mgr
            .add_observation("John checks email first thing", "morning_reflection")
            .await
            .unwrap();

        let config = test_config();
        let result = StateCompiler::new(&config)
            .compile_for_trigger(&TriggerType::MorningBriefing, &ledger, &state_mgr, None, None, None, None, None, None)
            .await;

        assert!(result.contains("John"));
        assert!(result.contains("Recent Activity"));
        assert!(result.contains("Dinner tomorrow"));
        assert!(result.contains("Memories"));
        assert!(result.contains("fish stew"));
        assert!(result.contains("Observations"));
        assert!(result.contains("checks email"));
    }

    #[tokio::test]
    async fn compile_for_trigger_email_searches_sender() {
        let (pool, _dir) = test_db().await;
        let ledger = Ledger::new(pool.clone());
        let state_mgr = StateManager::new(pool);

        // Seed history with this sender
        ledger
            .append(&Ledger::entry(
                sentinel_memory::ledger::LedgerCategory::EmailReceived,
                "From: boss@work.com — Q3 targets".into(),
                sentinel_memory::ledger::LedgerSource::Watcher("email".into()),
            ))
            .await
            .unwrap();

        ledger
            .append(&Ledger::entry(
                sentinel_memory::ledger::LedgerCategory::EmailReceived,
                "From: other@example.com — Newsletter".into(),
                sentinel_memory::ledger::LedgerSource::Watcher("email".into()),
            ))
            .await
            .unwrap();

        let config = test_config();
        let trigger = TriggerType::EmailTriage(crate::triage::EmailTrigger {
            from: "boss@work.com".into(),
            subject: "Q4 planning".into(),
            preview: "Let's discuss...".into(),
        });

        let result = StateCompiler::new(&config)
            .compile_for_trigger(&trigger, &ledger, &state_mgr, None, None, None, None, None, None)
            .await;

        // Should have related history for this sender
        assert!(result.contains("Related History"));
        assert!(result.contains("boss@work.com"));
        // Should also have recent activity
        assert!(result.contains("Recent Activity"));
    }

    #[tokio::test]
    async fn compile_for_trigger_empty_db() {
        let (pool, _dir) = test_db().await;
        let ledger = Ledger::new(pool.clone());
        let state_mgr = StateManager::new(pool);

        let config = test_config();
        let result = StateCompiler::new(&config)
            .compile_for_trigger(&TriggerType::MorningBriefing, &ledger, &state_mgr, None, None, None, None, None, None)
            .await;

        // Should still have the basic context header
        assert!(result.contains("John"));
        assert!(result.contains("Europe/Lisbon"));
        // No data sections when DB is empty
        assert!(!result.contains("Recent Activity"));
        assert!(!result.contains("Memories"));
    }

    #[tokio::test]
    async fn compile_household_in_morning_briefing() {
        let (pool, _dir) = test_db().await;
        let ledger = Ledger::new(pool.clone());
        let state_mgr = StateManager::new(pool.clone());

        // Household store
        let hh = sentinel_memory::household::HouseholdStore::new(pool, "john".into());

        // Add a meal for today
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        hh.add_meal(&sentinel_core::types::MealEntry {
            date: today,
            meal_type: "dinner".into(),
            description: "Bacalhau à Brás".into(),
            ingredients: vec!["bacalhau".into(), "potatoes".into()],
            created_by: "john".into(),
        }).await.unwrap();

        // Add a shopping item
        hh.add_shopping_item("Azeite", Some("Condiments"), None).await.unwrap();

        let config = test_config();
        let result = StateCompiler::new(&config)
            .compile_for_trigger(
                &TriggerType::MorningBriefing,
                &ledger, &state_mgr, None, None, None, None,
                Some(&hh),
                None,
            ).await;

        assert!(result.contains("Household"));
        assert!(result.contains("Bacalhau à Brás"));
        assert!(result.contains("Azeite"));
    }

    #[tokio::test]
    async fn compile_reflection_includes_rejections() {
        let (pool, _dir) = test_db().await;
        let ledger = Ledger::new(pool.clone());
        let state_mgr = StateManager::new(pool.clone());
        let audit = sentinel_membrane::audit::AuditLog::new(pool);

        // Record a rejection
        let entry = sentinel_membrane::audit::AuditEntry::new(
            &sentinel_core::capability::Capability::TaskListRead,
            sentinel_core::types::ActionSource::Cortex,
            sentinel_core::types::Decision::HumanRejected,
            "draft was inaccurate".into(),
        );
        audit.record(&entry).await.unwrap();

        let config = test_config();
        let result = StateCompiler::new(&config)
            .compile_for_trigger(
                &TriggerType::WeeklyReflection,
                &ledger, &state_mgr, None, None, None, None, None,
                Some(&audit),
            ).await;

        assert!(result.contains("Recent Corrections"));
        assert!(result.contains("draft was inaccurate"));
    }

    #[tokio::test]
    async fn compile_engagement_quiet_annotates_state() {
        let (pool, _dir) = test_db().await;
        let ledger = Ledger::new(pool.clone());
        let state_mgr = StateManager::new(pool);

        // Record an interaction 3 days ago (should be Quiet)
        state_mgr.record_interaction().await.unwrap();
        // Manually set the timestamp far back — use direct SQL
        let old_time = (Utc::now() - chrono::Duration::hours(72)).to_rfc3339();
        sqlx::query("UPDATE watcher_state SET state_value = ? WHERE state_key = 'last_interaction'")
            .bind(&old_time)
            .execute(state_mgr.pool())
            .await
            .unwrap();

        let config = test_config();
        let result = StateCompiler::new(&config)
            .compile_for_trigger(
                &TriggerType::MorningBriefing,
                &ledger, &state_mgr, None, None, None, None, None, None,
            ).await;

        assert!(result.contains("Engagement"));
        assert!(result.contains("Lower notification volume"));
    }
}
