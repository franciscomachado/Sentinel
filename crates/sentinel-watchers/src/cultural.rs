/// Cultural events engine with taste profile matching.
///
/// Fetches events from RSS/Atom feeds and iCal URLs, scores them against
/// the user's taste profile, and surfaces high-match events in briefings.
/// No AI cost — all scoring is local keyword/category matching.

use std::path::PathBuf;

use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use serde::{Deserialize, Serialize};
use tracing;

/// Source for cultural events.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum EventSource {
    Feed { name: String, url: String, refresh_hours: Option<u32> },
    #[serde(rename = "ical")]
    ICal { name: String, url: String, refresh_hours: Option<u32> },
    LocalFile { name: String, path: PathBuf },
}

/// A raw cultural event parsed from a source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CulturalEvent {
    pub title: String,
    pub description: Option<String>,
    pub venue: Option<String>,
    pub date: Option<DateTime<Utc>>,
    pub end_date: Option<DateTime<Utc>>,
    pub url: Option<String>,
    pub source_name: String,
    pub categories: Vec<String>,
}

/// A cultural event scored against the user's taste profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredEvent {
    pub event: CulturalEvent,
    /// 0.0–1.0 from taste profile keyword matching.
    pub interest_score: f64,
    /// 0.0–1.0 from calendar/context feasibility (simplified: just time-based).
    pub feasibility_score: f64,
    /// Combined score.
    pub combined: f64,
}

/// User's taste profile for cultural events.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TasteProfile {
    /// Keywords/categories the user likes.
    pub likes: Vec<String>,
    /// Keywords they might enjoy.
    pub maybe: Vec<String>,
    /// Keywords they're not interested in.
    pub not_interested: Vec<String>,
    /// Learned preferences from interaction feedback.
    #[serde(default)]
    pub learned: Vec<LearnedPreference>,
}

/// A learned preference from past interactions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnedPreference {
    pub keyword: String,
    /// Positive = liked, negative = dismissed.
    pub weight: f64,
}

pub struct CulturalEventsWatcher {
    pub sources: Vec<EventSource>,
    pub taste: TasteProfile,
    pub check_interval_hours: u32,
    http: reqwest::Client,
}

impl CulturalEventsWatcher {
    pub fn new(
        sources: Vec<EventSource>,
        taste: TasteProfile,
        check_interval_hours: u32,
    ) -> Self {
        Self {
            sources,
            taste,
            check_interval_hours,
            http: reqwest::Client::new(),
        }
    }

    /// Fetch and score all events from configured sources.
    pub async fn fetch_and_score(&self) -> Vec<ScoredEvent> {
        let mut all_events = Vec::new();

        for source in &self.sources {
            match source {
                EventSource::Feed { name, url, .. } => {
                    match self.fetch_feed(url, name).await {
                        Ok(events) => all_events.extend(events),
                        Err(e) => tracing::warn!(source = %name, error = %e, "failed to fetch feed"),
                    }
                }
                EventSource::ICal { name, url, .. } => {
                    match self.fetch_ical(url, name).await {
                        Ok(events) => all_events.extend(events),
                        Err(e) => tracing::warn!(source = %name, error = %e, "failed to fetch ical"),
                    }
                }
                EventSource::LocalFile { name, path } => {
                    match load_local_events(path, name) {
                        Ok(events) => all_events.extend(events),
                        Err(e) => tracing::warn!(source = %name, error = %e, "failed to load local events"),
                    }
                }
            }
        }

        // Filter out past events
        let now = Utc::now();
        all_events.retain(|e| {
            e.date.map(|d| d > now).unwrap_or(true)
        });

        // Score and sort
        let mut scored: Vec<ScoredEvent> = all_events
            .into_iter()
            .map(|e| self.score_event(e))
            .filter(|s| s.combined > 0.0) // Drop zero-score (not_interested matches)
            .collect();

        scored.sort_by(|a, b| b.combined.partial_cmp(&a.combined).unwrap_or(std::cmp::Ordering::Equal));
        scored
    }

    /// Get top N scored events.
    pub async fn top_events(&self, n: usize) -> Vec<ScoredEvent> {
        let scored = self.fetch_and_score().await;
        scored.into_iter().take(n).collect()
    }

    /// Run a polling loop that emits high-scoring cultural events as WatchEvents.
    pub async fn run(
        &self,
        tx: tokio::sync::mpsc::Sender<sentinel_core::events::WatchEvent>,
        top_n: usize,
    ) -> anyhow::Result<()> {
        use sentinel_core::events::{CulturalAlert, WatchEvent};

        let interval = std::time::Duration::from_secs(self.check_interval_hours as u64 * 3600);
        loop {
            let top = self.top_events(top_n).await;
            for scored in &top {
                let alert = CulturalAlert {
                    title: scored.event.title.clone(),
                    venue: scored.event.venue.clone(),
                    date: scored.event.date,
                    source_name: scored.event.source_name.clone(),
                    match_score: scored.combined,
                };
                if tx.send(WatchEvent::Cultural(alert)).await.is_err() {
                    return Ok(());
                }
            }
            tracing::info!(count = top.len(), "cultural events check complete");
            tokio::time::sleep(interval).await;
        }
    }

    /// Score a single event against the taste profile.
    fn score_event(&self, event: CulturalEvent) -> ScoredEvent {
        let text = format!(
            "{} {} {}",
            event.title,
            event.description.as_deref().unwrap_or(""),
            event.categories.join(" "),
        ).to_lowercase();

        // Check not_interested first — if matched, score is 0
        for keyword in &self.taste.not_interested {
            if text.contains(&keyword.to_lowercase()) {
                return ScoredEvent {
                    event,
                    interest_score: 0.0,
                    feasibility_score: 0.0,
                    combined: 0.0,
                };
            }
        }

        let mut interest = 0.0f64;

        // Likes: +0.4 per match
        for keyword in &self.taste.likes {
            if text.contains(&keyword.to_lowercase()) {
                interest += 0.4;
            }
        }

        // Maybe: +0.2 per match
        for keyword in &self.taste.maybe {
            if text.contains(&keyword.to_lowercase()) {
                interest += 0.2;
            }
        }

        // Learned preferences
        for pref in &self.taste.learned {
            if text.contains(&pref.keyword.to_lowercase()) {
                interest += pref.weight * 0.3;
            }
        }

        let interest_score = interest.clamp(0.0, 1.0);

        // Simple feasibility: events within 7 days score higher
        let feasibility_score = if let Some(date) = event.date {
            let days_away = (date - Utc::now()).num_days();
            if days_away < 0 {
                0.0
            } else if days_away <= 3 {
                1.0
            } else if days_away <= 7 {
                0.8
            } else if days_away <= 14 {
                0.5
            } else {
                0.3
            }
        } else {
            0.5 // Unknown date → neutral
        };

        let combined = interest_score * 0.6 + feasibility_score * 0.4;

        ScoredEvent {
            event,
            interest_score,
            feasibility_score,
            combined,
        }
    }

    /// Fetch and parse an RSS/Atom feed.
    async fn fetch_feed(&self, url: &str, source_name: &str) -> anyhow::Result<Vec<CulturalEvent>> {
        let body = self.http.get(url).send().await?.text().await?;
        Ok(parse_rss_simple(&body, source_name))
    }

    /// Fetch and parse an iCal feed.
    async fn fetch_ical(&self, url: &str, source_name: &str) -> anyhow::Result<Vec<CulturalEvent>> {
        let body = self.http.get(url).send().await?.text().await?;
        Ok(parse_ical_simple(&body, source_name))
    }
}

/// Minimal RSS/Atom parser — extracts title, description, link, pubDate/updated, category.
/// No external XML library needed; uses simple string scanning.
fn parse_rss_simple(xml: &str, source_name: &str) -> Vec<CulturalEvent> {
    let mut events = Vec::new();

    // Try RSS <item> first, then Atom <entry>
    let item_tag = if xml.contains("<item>") || xml.contains("<item ") {
        "item"
    } else if xml.contains("<entry>") || xml.contains("<entry ") {
        "entry"
    } else {
        return events;
    };

    for item_content in extract_tags(xml, item_tag) {
        let title = extract_first_tag_content(&item_content, "title")
            .unwrap_or_default();
        if title.is_empty() {
            continue;
        }

        let description = extract_first_tag_content(&item_content, "description")
            .or_else(|| extract_first_tag_content(&item_content, "summary"))
            .or_else(|| extract_first_tag_content(&item_content, "content"));

        let url = extract_first_tag_content(&item_content, "link")
            .or_else(|| extract_href_attr(&item_content, "link"));

        let date = extract_first_tag_content(&item_content, "pubDate")
            .or_else(|| extract_first_tag_content(&item_content, "updated"))
            .or_else(|| extract_first_tag_content(&item_content, "published"))
            .and_then(|d| parse_flexible_date(&d));

        let mut categories = Vec::new();
        for cat in extract_tags(&item_content, "category") {
            let cat = cat.trim().to_string();
            if !cat.is_empty() {
                categories.push(cat);
            }
        }

        events.push(CulturalEvent {
            title: decode_html_entities(&title),
            description: description.map(|d| decode_html_entities(&d)),
            venue: None,
            date,
            end_date: None,
            url,
            source_name: source_name.to_string(),
            categories,
        });
    }

    events
}

/// Minimal iCal parser — extracts VEVENT blocks.
fn parse_ical_simple(ical: &str, source_name: &str) -> Vec<CulturalEvent> {
    let mut events = Vec::new();

    for vevent in ical.split("BEGIN:VEVENT") {
        if !vevent.contains("END:VEVENT") {
            continue;
        }

        let block = vevent.split("END:VEVENT").next().unwrap_or("");

        let title = ical_prop(block, "SUMMARY").unwrap_or_default();
        if title.is_empty() {
            continue;
        }

        let description = ical_prop(block, "DESCRIPTION");
        let venue = ical_prop(block, "LOCATION");
        let url = ical_prop(block, "URL");

        let date = ical_prop(block, "DTSTART")
            .and_then(|d| parse_ical_datetime(&d));
        let end_date = ical_prop(block, "DTEND")
            .and_then(|d| parse_ical_datetime(&d));

        let categories = ical_prop(block, "CATEGORIES")
            .map(|c| c.split(',').map(|s| s.trim().to_string()).collect())
            .unwrap_or_default();

        events.push(CulturalEvent {
            title,
            description,
            venue,
            date,
            end_date,
            url,
            source_name: source_name.to_string(),
            categories,
        });
    }

    events
}

/// Load events from a local TOML file.
fn load_local_events(path: &PathBuf, source_name: &str) -> anyhow::Result<Vec<CulturalEvent>> {
    let content = std::fs::read_to_string(path)?;

    #[derive(Deserialize)]
    struct LocalEvents {
        #[serde(default)]
        events: Vec<LocalEvent>,
    }

    #[derive(Deserialize)]
    struct LocalEvent {
        title: String,
        description: Option<String>,
        venue: Option<String>,
        date: Option<String>,
        url: Option<String>,
        #[serde(default)]
        categories: Vec<String>,
    }

    let data: LocalEvents = toml::from_str(&content)?;

    Ok(data.events.into_iter().map(|e| {
        CulturalEvent {
            title: e.title,
            description: e.description,
            venue: e.venue,
            date: e.date.and_then(|d| parse_flexible_date(&d)),
            end_date: None,
            url: e.url,
            source_name: source_name.to_string(),
            categories: e.categories,
        }
    }).collect())
}

/// Format scored events for the state compiler.
pub fn format_cultural_events(events: &[ScoredEvent]) -> String {
    if events.is_empty() {
        return String::new();
    }
    let mut lines = Vec::new();
    for se in events {
        let date_str = se.event.date
            .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "date TBD".into());
        let venue_str = se.event.venue
            .as_deref()
            .map(|v| format!(" at {v}"))
            .unwrap_or_default();
        let score_str = format!("({:.0}% match)", se.combined * 100.0);
        lines.push(format!(
            "- {}{venue_str} — {date_str} {score_str}",
            se.event.title,
        ));
    }
    lines.join("\n")
}

// ── Helper functions ──

fn extract_tags<'a>(xml: &'a str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let open_attr = format!("<{tag} ");
    let close = format!("</{tag}>");
    let mut results = Vec::new();
    let mut search_from = 0;

    while search_from < xml.len() {
        let start = xml[search_from..].find(&open)
            .map(|i| i + open.len())
            .or_else(|| {
                xml[search_from..].find(&open_attr).and_then(|i| {
                    xml[search_from + i..].find('>').map(|j| i + j + 1)
                })
            });

        let Some(start) = start else { break };
        let abs_start = search_from + start;

        let Some(end) = xml[abs_start..].find(&close) else { break };
        let abs_end = abs_start + end;

        results.push(xml[abs_start..abs_end].to_string());
        search_from = abs_end + close.len();
    }

    results
}

fn extract_first_tag_content(xml: &str, tag: &str) -> Option<String> {
    extract_tags(xml, tag).into_iter().next()
}

fn extract_href_attr(xml: &str, tag: &str) -> Option<String> {
    let pattern = format!("<{tag} ");
    let start = xml.find(&pattern)?;
    let rest = &xml[start..];
    let end = rest.find("/>")?;
    let tag_str = &rest[..end];
    let href_start = tag_str.find("href=\"")? + 6;
    let href_end = tag_str[href_start..].find('"')?;
    Some(tag_str[href_start..href_start + href_end].to_string())
}

fn decode_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

fn parse_flexible_date(s: &str) -> Option<DateTime<Utc>> {
    // RFC 2822 (RSS pubDate)
    if let Ok(dt) = DateTime::parse_from_rfc2822(s.trim()) {
        return Some(dt.with_timezone(&Utc));
    }
    // RFC 3339 / ISO 8601
    if let Ok(dt) = DateTime::parse_from_rfc3339(s.trim()) {
        return Some(dt.with_timezone(&Utc));
    }
    // ISO date only
    if let Ok(d) = NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d") {
        return Some(d.and_time(NaiveTime::from_hms_opt(0, 0, 0)?).and_utc());
    }
    None
}

fn parse_ical_datetime(s: &str) -> Option<DateTime<Utc>> {
    // Handle VALUE=DATE: prefix or TZID= prefix
    let value = s.split(':').last().unwrap_or(s).trim();

    // Basic format: 20260315T140000Z
    if let Ok(dt) = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%SZ") {
        return Some(dt.and_utc());
    }
    // Without Z (floating)
    if let Ok(dt) = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S") {
        return Some(dt.and_utc());
    }
    // Date only: 20260315
    if let Ok(d) = NaiveDate::parse_from_str(value, "%Y%m%d") {
        return Some(d.and_time(NaiveTime::from_hms_opt(0, 0, 0)?).and_utc());
    }
    None
}

fn ical_prop(block: &str, prop: &str) -> Option<String> {
    for line in block.lines() {
        let trimmed = line.trim();
        // Property can be "SUMMARY:value" or "SUMMARY;param=x:value"
        if trimmed.starts_with(prop) {
            let rest = &trimmed[prop.len()..];
            if let Some(stripped) = rest.strip_prefix(':') {
                return Some(stripped.to_string());
            }
            if rest.starts_with(';') {
                // Has parameters — find the colon after them
                if let Some(colon_pos) = rest.find(':') {
                    return Some(rest[colon_pos + 1..].to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    #[test]
    fn parse_rss_feed() {
        let rss = r#"<?xml version="1.0"?>
<rss version="2.0">
<channel>
<title>Local Events</title>
<item>
<title>Jazz Night at Casa da Música</title>
<description>Miles Davis tribute concert</description>
<link>https://example.com/jazz</link>
<pubDate>Wed, 18 Mar 2026 20:00:00 +0000</pubDate>
<category>music</category>
<category>jazz</category>
</item>
<item>
<title>Art Exhibition: Modern Porto</title>
<description>Contemporary art from local artists</description>
<link>https://example.com/art</link>
<pubDate>Sun, 15 Mar 2026 10:00:00 +0000</pubDate>
<category>art</category>
</item>
</channel>
</rss>"#;

        let events = parse_rss_simple(rss, "Test Venue");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].title, "Jazz Night at Casa da Música");
        assert!(events[0].date.is_some());
        assert_eq!(events[0].categories, vec!["music", "jazz"]);
    }

    #[test]
    fn parse_ical_events() {
        let ical = r#"BEGIN:VCALENDAR
BEGIN:VEVENT
SUMMARY:Food Festival Porto
DTSTART:20260320T120000Z
DTEND:20260320T220000Z
LOCATION:Parque da Cidade
DESCRIPTION:Annual food and wine festival
CATEGORIES:food,festival
URL:https://example.com/food-fest
END:VEVENT
BEGIN:VEVENT
SUMMARY:Opera Night
DTSTART:20260322T200000Z
LOCATION:Teatro Nacional
CATEGORIES:opera,music
END:VEVENT
END:VCALENDAR"#;

        let events = parse_ical_simple(ical, "Porto Events");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].title, "Food Festival Porto");
        assert_eq!(events[0].venue.as_deref(), Some("Parque da Cidade"));
        assert!(events[0].date.is_some());
        assert_eq!(events[1].title, "Opera Night");
    }

    #[test]
    fn taste_scoring() {
        let taste = TasteProfile {
            likes: vec!["jazz".into(), "food festival".into()],
            maybe: vec!["art".into(), "theatre".into()],
            not_interested: vec!["opera".into(), "nightclub".into()],
            learned: vec![],
        };

        let watcher = CulturalEventsWatcher::new(vec![], taste, 24);

        // Jazz event → high score (matches "likes")
        let jazz = CulturalEvent {
            title: "Jazz Night".into(),
            description: Some("Live jazz performance".into()),
            venue: None,
            date: Some(Utc::now() + chrono::Duration::days(2)),
            end_date: None,
            url: None,
            source_name: "test".into(),
            categories: vec!["music".into(), "jazz".into()],
        };
        let scored = watcher.score_event(jazz);
        assert!(scored.interest_score > 0.3, "jazz should score high, got {}", scored.interest_score);
        assert!(scored.combined > 0.2);

        // Art event → medium score (matches "maybe")
        let art = CulturalEvent {
            title: "Art Exhibition".into(),
            description: None,
            venue: None,
            date: Some(Utc::now() + chrono::Duration::days(5)),
            end_date: None,
            url: None,
            source_name: "test".into(),
            categories: vec!["art".into()],
        };
        let scored = watcher.score_event(art);
        assert!(scored.interest_score > 0.0);
        assert!(scored.interest_score < 0.4);

        // Opera → zero (not_interested)
        let opera = CulturalEvent {
            title: "Opera Night".into(),
            description: Some("Carmen at the National Theatre".into()),
            venue: None,
            date: Some(Utc::now() + chrono::Duration::days(3)),
            end_date: None,
            url: None,
            source_name: "test".into(),
            categories: vec!["opera".into()],
        };
        let scored = watcher.score_event(opera);
        assert_eq!(scored.combined, 0.0, "opera should be filtered out");
    }

    #[test]
    fn format_scored_events_output() {
        let events = vec![
            ScoredEvent {
                event: CulturalEvent {
                    title: "Jazz Night".into(),
                    description: None,
                    venue: Some("Casa da Música".into()),
                    date: Some(Utc::now() + chrono::Duration::days(2)),
                    end_date: None,
                    url: None,
                    source_name: "test".into(),
                    categories: vec![],
                },
                interest_score: 0.8,
                feasibility_score: 1.0,
                combined: 0.88,
            },
        ];

        let formatted = format_cultural_events(&events);
        assert!(formatted.contains("Jazz Night"));
        assert!(formatted.contains("Casa da Música"));
        assert!(formatted.contains("88% match"));
    }

    #[test]
    fn ical_datetime_parsing() {
        // UTC
        let dt = parse_ical_datetime("20260315T140000Z").unwrap();
        assert_eq!(dt.hour(), 14);

        // Floating
        let dt = parse_ical_datetime("20260315T200000").unwrap();
        assert_eq!(dt.hour(), 20);

        // Date only
        let dt = parse_ical_datetime("20260315").unwrap();
        assert_eq!(dt.hour(), 0);

        // With TZID prefix
        let dt = parse_ical_datetime("TZID=Europe/Lisbon:20260315T200000").unwrap();
        assert_eq!(dt.hour(), 20);
    }
}
