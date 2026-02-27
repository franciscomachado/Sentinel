/// Sports calendar watcher for motorsport and other events.
///
/// Loads community-contributed TOML data files at startup, converts session times
/// to UTC, and emits SportsAlert events when sessions are about to start.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing;

use sentinel_core::config::SportsSeriesConfig;
use sentinel_core::events::{SportsAlert, SportsEvent, WatchEvent};

/// Parsed TOML data file for a season.
#[derive(Debug, Deserialize)]
pub struct SeasonData {
    pub series: SeriesMeta,
    #[serde(default)]
    pub rounds: Vec<Round>,
}

#[derive(Debug, Deserialize)]
pub struct SeriesMeta {
    pub name: String,
    pub id: String,
    pub season: u32,
}

#[derive(Debug, Deserialize)]
pub struct Round {
    pub round: u32,
    pub name: String,
    pub circuit: Option<String>,
    pub city: Option<String>,
    pub country: Option<String>,
    pub timezone: Option<String>,
    pub date_start: Option<String>,
    pub date_end: Option<String>,
    #[serde(default)]
    pub sessions: Vec<Session>,
}

#[derive(Debug, Deserialize)]
pub struct Session {
    pub name: String,
    /// Date as "YYYY-MM-DD"
    pub day: String,
    /// Time as "HH:MM" (local to circuit timezone)
    pub time: String,
    #[serde(default)]
    pub timezone: Option<String>,
}

/// A resolved session with UTC time.
#[derive(Debug, Clone)]
pub struct ResolvedSession {
    pub series_id: String,
    pub series_name: String,
    pub round_name: String,
    pub session_name: String,
    pub start_utc: DateTime<Utc>,
    pub spoiler_protect: bool,
}

pub struct SportsCalendarWatcher {
    pub data_dir: PathBuf,
    /// User's configured series with interest levels.
    series_configs: Vec<SportsSeriesConfig>,
}

impl SportsCalendarWatcher {
    pub fn new(data_dir: PathBuf, series_configs: Vec<SportsSeriesConfig>) -> Self {
        Self { data_dir, series_configs }
    }

    /// Load all season data files and resolve sessions.
    pub fn load_sessions(&self) -> Vec<ResolvedSession> {
        let mut sessions = Vec::new();

        // Build a map of series id → config for quick lookup
        let config_map: std::collections::HashMap<&str, &SportsSeriesConfig> =
            self.series_configs.iter().map(|c| (c.id.as_str(), c)).collect();

        // Walk the data directory for TOML files
        let entries = match std::fs::read_dir(&self.data_dir) {
            Ok(entries) => entries,
            Err(e) => {
                tracing::warn!(dir = %self.data_dir.display(), error = %e, "failed to read sports data dir");
                // Try subdirectories (data/sports/motorsport/, etc.)
                return self.load_from_subdirs(&config_map);
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // Recurse into subdirectories (motorsport/, football/, etc.)
                if let Ok(sub_entries) = std::fs::read_dir(&path) {
                    for sub_entry in sub_entries.flatten() {
                        let sub_path = sub_entry.path();
                        self.load_file(&sub_path, &config_map, &mut sessions);
                    }
                }
            } else {
                self.load_file(&path, &config_map, &mut sessions);
            }
        }

        sessions.sort_by_key(|s| s.start_utc);
        sessions
    }

    fn load_from_subdirs(
        &self,
        config_map: &std::collections::HashMap<&str, &SportsSeriesConfig>,
    ) -> Vec<ResolvedSession> {
        let mut sessions = Vec::new();
        // Try common subdirectory names
        for subdir in &["motorsport", "football", "tennis"] {
            let dir = self.data_dir.join(subdir);
            if dir.is_dir() {
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    for entry in entries.flatten() {
                        self.load_file(&entry.path(), config_map, &mut sessions);
                    }
                }
            }
        }
        sessions.sort_by_key(|s| s.start_utc);
        sessions
    }

    fn load_file(
        &self,
        path: &Path,
        config_map: &std::collections::HashMap<&str, &SportsSeriesConfig>,
        sessions: &mut Vec<ResolvedSession>,
    ) {
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            return;
        }

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "failed to read sports data file");
                return;
            }
        };

        let data: SeasonData = match toml::from_str(&content) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "failed to parse sports data file");
                return;
            }
        };

        // Only load series the user is interested in
        let series_config = config_map.get(data.series.id.as_str());
        let spoiler_protect = series_config.map(|c| c.spoiler_protect).unwrap_or(false);

        // If user has no config for this series, skip it
        if series_config.is_none() {
            return;
        }

        for round in &data.rounds {
            let round_tz = round.timezone.as_deref();
            for session in &round.sessions {
                let tz = session.timezone.as_deref().or(round_tz);
                if let Some(start_utc) = parse_session_time(&session.day, &session.time, tz) {
                    sessions.push(ResolvedSession {
                        series_id: data.series.id.clone(),
                        series_name: data.series.name.clone(),
                        round_name: round.name.clone(),
                        session_name: session.name.clone(),
                        start_utc,
                        spoiler_protect,
                    });
                }
            }
        }

        tracing::debug!(
            series = %data.series.name,
            rounds = data.rounds.len(),
            path = %path.display(),
            "loaded sports data"
        );
    }

    /// Run the sports watcher event loop.
    /// Emits SportsAlert events 60 minutes before each session starts.
    pub async fn run(&self, tx: mpsc::Sender<WatchEvent>) -> anyhow::Result<()> {
        let mut sessions = self.load_sessions();
        let mut alerted: HashSet<String> = HashSet::new();

        // Build notify policy from config
        let notify_policy: std::collections::HashMap<&str, &str> =
            self.series_configs.iter().map(|c| (c.id.as_str(), c.notify.as_str())).collect();

        loop {
            let now = Utc::now();

            // Remove past sessions (more than 3 hours ago)
            sessions.retain(|s| s.start_utc > now - chrono::Duration::hours(3));

            for session in &sessions {
                let alert_key = format!(
                    "{}:{}:{}",
                    session.series_id, session.round_name, session.session_name
                );
                if alerted.contains(&alert_key) {
                    continue;
                }

                let minutes_until = (session.start_utc - now).num_minutes();

                // Alert 60 minutes before
                if minutes_until <= 60 && minutes_until > -30 {
                    // Check notify policy
                    let policy = notify_policy.get(session.series_id.as_str()).copied().unwrap_or("each_session");
                    let should_notify = match policy {
                        "race_only" => session.session_name.to_lowercase().contains("race"),
                        "weekly_mention" => false, // handled by state compiler in briefings
                        _ => true, // "each_session"
                    };

                    if should_notify {
                        let alert = SportsAlert {
                            series_id: session.series_id.clone(),
                            series_name: session.series_name.clone(),
                            round_name: session.round_name.clone(),
                            session_name: session.session_name.clone(),
                            start_utc: session.start_utc,
                            spoiler_protect: session.spoiler_protect,
                        };
                        if tx.send(WatchEvent::Sports(alert)).await.is_err() {
                            return Ok(());
                        }
                    }
                    alerted.insert(alert_key);
                }
            }

            // Sleep for 5 minutes between checks
            tokio::time::sleep(std::time::Duration::from_secs(300)).await;
        }
    }
}

/// Get upcoming sessions within `days` from now, resolved to user timezone.
pub fn upcoming_sessions(
    sessions: &[ResolvedSession],
    days: u32,
    tz_offset: chrono::FixedOffset,
) -> Vec<SportsEvent> {
    let now = Utc::now();
    let cutoff = now + chrono::Duration::days(days as i64);

    sessions
        .iter()
        .filter(|s| s.start_utc > now && s.start_utc < cutoff)
        .map(|s| {
            let local = s.start_utc.with_timezone(&tz_offset);
            SportsEvent {
                series_id: s.series_id.clone(),
                series_name: s.series_name.clone(),
                round_name: s.round_name.clone(),
                session_name: s.session_name.clone(),
                start_utc: s.start_utc,
                local_time: local.format("%H:%M").to_string(),
                local_date: local.format("%Y-%m-%d").to_string(),
                spoiler_protect: s.spoiler_protect,
            }
        })
        .collect()
}

/// Get sessions happening today, resolved to user timezone.
pub fn today_sessions(
    sessions: &[ResolvedSession],
    tz_offset: chrono::FixedOffset,
) -> Vec<SportsEvent> {
    let now = Utc::now();
    let local_today = now.with_timezone(&tz_offset).date_naive();

    sessions
        .iter()
        .filter(|s| {
            let local = s.start_utc.with_timezone(&tz_offset);
            local.date_naive() == local_today
        })
        .map(|s| {
            let local = s.start_utc.with_timezone(&tz_offset);
            SportsEvent {
                series_id: s.series_id.clone(),
                series_name: s.series_name.clone(),
                round_name: s.round_name.clone(),
                session_name: s.session_name.clone(),
                start_utc: s.start_utc,
                local_time: local.format("%H:%M").to_string(),
                local_date: local.format("%Y-%m-%d").to_string(),
                spoiler_protect: s.spoiler_protect,
            }
        })
        .collect()
}

/// Parse a session time from day + time + timezone into UTC.
/// Uses a simple offset map for common IANA timezone names.
fn parse_session_time(day: &str, time: &str, tz: Option<&str>) -> Option<DateTime<Utc>> {
    let date = NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
    let time = NaiveTime::parse_from_str(time, "%H:%M").ok()?;
    let naive = NaiveDateTime::new(date, time);

    let offset = tz.map(tz_name_to_offset).unwrap_or(0);
    let fixed = chrono::FixedOffset::east_opt(offset * 3600)?;
    Some(fixed.from_local_datetime(&naive).single()?.to_utc())
}

/// Map IANA timezone names to UTC offset in hours.
/// This is a simplified static mapping; for DST-accurate conversion,
/// use chrono-tz (not in deps to keep binary minimal).
fn tz_name_to_offset(name: &str) -> i32 {
    match name {
        "Australia/Melbourne" | "Australia/Sydney" => 11,
        "Australia/Adelaide" => 10,
        "Australia/Perth" => 8,
        "Asia/Tokyo" => 9,
        "Asia/Shanghai" | "Asia/Hong_Kong" | "Asia/Singapore" => 8,
        "Asia/Kolkata" | "Asia/Calcutta" => 5,
        "Asia/Dubai" => 4,
        "Europe/Moscow" => 3,
        "Europe/Helsinki" | "Europe/Bucharest" | "Europe/Athens" => 2,
        "Europe/Paris" | "Europe/Berlin" | "Europe/Madrid" | "Europe/Rome"
        | "Europe/Amsterdam" | "Europe/Monaco" | "Europe/Vienna" => 1,
        "Europe/Lisbon" | "Europe/London" | "Atlantic/Canary" => 0,
        "America/Sao_Paulo" => -3,
        "America/New_York" | "US/Eastern" => -5,
        "America/Chicago" | "US/Central" | "America/Mexico_City" => -6,
        "America/Denver" | "US/Mountain" => -7,
        "America/Los_Angeles" | "US/Pacific" => -8,
        "Pacific/Auckland" => 12,
        "UTC" | "Etc/UTC" => 0,
        _ => {
            tracing::warn!(tz = name, "unknown sports timezone, assuming UTC");
            0
        }
    }
}

/// Format sports events for state compiler output.
pub fn format_sports_events(events: &[SportsEvent]) -> String {
    if events.is_empty() {
        return String::new();
    }
    let mut lines = Vec::with_capacity(events.len());
    for ev in events {
        let spoiler = if ev.spoiler_protect { " 🔇" } else { "" };
        lines.push(format!(
            "- {} {} — {} at {}{spoiler}",
            ev.series_name, ev.round_name, ev.session_name, ev.local_time,
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn sample_toml() -> &'static str {
        r#"
[series]
name = "Formula 1"
id = "f1"
season = 2026

[[rounds]]
round = 1
name = "Australian Grand Prix"
circuit = "Albert Park"
city = "Melbourne"
country = "Australia"
timezone = "Australia/Melbourne"
date_start = "2026-03-13"
date_end = "2026-03-15"

[[rounds.sessions]]
name = "Practice 1"
day = "2026-03-13"
time = "12:30"

[[rounds.sessions]]
name = "Race"
day = "2026-03-15"
time = "15:00"
"#
    }

    #[test]
    fn parse_season_data() {
        let data: SeasonData = toml::from_str(sample_toml()).unwrap();
        assert_eq!(data.series.id, "f1");
        assert_eq!(data.series.name, "Formula 1");
        assert_eq!(data.rounds.len(), 1);
        assert_eq!(data.rounds[0].sessions.len(), 2);
        assert_eq!(data.rounds[0].sessions[1].name, "Race");
    }

    #[test]
    fn resolve_session_time_to_utc() {
        use chrono::{Datelike, Timelike};
        let utc = parse_session_time("2026-03-15", "15:00", Some("Australia/Melbourne")).unwrap();
        // Melbourne is UTC+11(winter), so 15:00 AEDT → 04:00 UTC
        assert_eq!(utc.hour(), 4);
        assert_eq!(utc.day(), 15);
    }

    #[test]
    fn loads_sessions_from_files() {
        let dir = tempfile::tempdir().unwrap();
        let motor_dir = dir.path().join("motorsport");
        std::fs::create_dir_all(&motor_dir).unwrap();

        let mut f = std::fs::File::create(motor_dir.join("f1-2026.toml")).unwrap();
        f.write_all(sample_toml().as_bytes()).unwrap();

        let configs = vec![SportsSeriesConfig {
            id: "f1".into(),
            name: "Formula 1".into(),
            interest: "follow".into(),
            notify: "each_session".into(),
            spoiler_protect: false,
        }];

        let watcher = SportsCalendarWatcher::new(dir.path().to_path_buf(), configs);
        let sessions = watcher.load_sessions();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].series_name, "Formula 1");
        assert_eq!(sessions[0].round_name, "Australian Grand Prix");
    }

    #[test]
    fn upcoming_sessions_filter() {
        let now = Utc::now();
        let sessions = vec![
            ResolvedSession {
                series_id: "f1".into(),
                series_name: "Formula 1".into(),
                round_name: "Test GP".into(),
                session_name: "Race".into(),
                start_utc: now + chrono::Duration::hours(2),
                spoiler_protect: false,
            },
            ResolvedSession {
                series_id: "f1".into(),
                series_name: "Formula 1".into(),
                round_name: "Old GP".into(),
                session_name: "Race".into(),
                start_utc: now - chrono::Duration::days(1),
                spoiler_protect: false,
            },
        ];

        let tz = chrono::FixedOffset::east_opt(0).unwrap();
        let upcoming = upcoming_sessions(&sessions, 7, tz);
        assert_eq!(upcoming.len(), 1);
        assert_eq!(upcoming[0].round_name, "Test GP");
    }

    #[test]
    fn skips_unconfigured_series() {
        let dir = tempfile::tempdir().unwrap();
        let motor_dir = dir.path().join("motorsport");
        std::fs::create_dir_all(&motor_dir).unwrap();

        let mut f = std::fs::File::create(motor_dir.join("f1-2026.toml")).unwrap();
        f.write_all(sample_toml().as_bytes()).unwrap();

        // No configs → no sessions loaded
        let watcher = SportsCalendarWatcher::new(dir.path().to_path_buf(), vec![]);
        let sessions = watcher.load_sessions();
        assert!(sessions.is_empty());
    }
}
