use chrono::{DateTime, Utc};
use sentinel_core::capability::{CalendarEvent, EventId};
use sentinel_core::config::CalendarConfig;
use sentinel_core::events::{CalendarChange, WatchEvent};

/// CalDAV watcher with ctag/etag change detection.
///
/// Polls a CalDAV server (e.g. Radicale) for calendar changes using the
/// PROPFIND method. Detects changes via ctag (collection-level) and
/// etag (per-event) to minimize bandwidth. Only fetches changed events.
pub struct CalDavWatcher {
    config: CalendarConfig,
    http: reqwest::Client,
}

/// Internal representation of a CalDAV event with etag for change tracking.
#[derive(Debug, Clone)]
struct CalDavEvent {
    href: String,
    etag: String,
    event: CalendarEvent,
}

impl CalDavWatcher {
    pub fn new(config: CalendarConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");
        Self { config, http }
    }

    /// Run the watcher loop. Polls for calendar changes and sends them
    /// into the event channel.
    pub async fn run(&self, tx: tokio::sync::mpsc::Sender<WatchEvent>) -> anyhow::Result<()> {
        tracing::info!(url = %self.config.caldav_url, "CalDAV watcher starting");

        let poll_interval =
            std::time::Duration::from_secs(self.config.poll_interval_secs);
        let mut known_events: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut last_ctag: Option<String> = None;

        loop {
            match self.check_for_changes(&mut known_events, &mut last_ctag).await {
                Ok(changes) => {
                    for change in changes {
                        if tx.send(WatchEvent::Calendar(change)).await.is_err() {
                            tracing::info!("event channel closed, CalDAV watcher stopping");
                            return Ok(());
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "CalDAV poll failed, retrying next cycle");
                }
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Check the CalDAV server for changes since last poll.
    async fn check_for_changes(
        &self,
        known_events: &mut std::collections::HashMap<String, String>,
        last_ctag: &mut Option<String>,
    ) -> anyhow::Result<Vec<CalendarChange>> {
        // Step 1: Check collection ctag for quick "nothing changed" check
        let current_ctag = self.get_ctag().await?;
        if last_ctag.as_ref() == Some(&current_ctag) {
            tracing::debug!("CalDAV ctag unchanged, skipping");
            return Ok(vec![]);
        }

        // Step 2: Get all event hrefs + etags
        let events = self.list_events().await?;

        let mut changes = Vec::new();

        // Detect created/modified events
        let mut current_hrefs = std::collections::HashSet::new();
        for ev in &events {
            current_hrefs.insert(ev.href.clone());
            match known_events.get(&ev.href) {
                None => {
                    // New event
                    tracing::info!(href = %ev.href, "calendar event created");
                    changes.push(CalendarChange::Created(ev.event.clone()));
                    known_events.insert(ev.href.clone(), ev.etag.clone());
                }
                Some(old_etag) if old_etag != &ev.etag => {
                    // Modified event
                    tracing::info!(href = %ev.href, "calendar event modified");
                    changes.push(CalendarChange::Modified(ev.event.clone()));
                    known_events.insert(ev.href.clone(), ev.etag.clone());
                }
                _ => {} // Unchanged
            }
        }

        // Detect deleted events
        let deleted: Vec<String> = known_events
            .keys()
            .filter(|k| !current_hrefs.contains(*k))
            .cloned()
            .collect();
        for href in deleted {
            tracing::info!(href = %href, "calendar event deleted");
            changes.push(CalendarChange::Deleted(EventId(href.clone())));
            known_events.remove(&href);
        }

        *last_ctag = Some(current_ctag);

        if !changes.is_empty() {
            tracing::info!(count = changes.len(), "calendar changes detected");
        }

        Ok(changes)
    }

    /// Get the collection ctag (CalDAV change tag).
    async fn get_ctag(&self) -> anyhow::Result<String> {
        let body = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:cs="http://calendarserver.org/ns/">
  <d:prop>
    <cs:getctag/>
  </d:prop>
</d:propfind>"#;

        let resp = self
            .request("PROPFIND", &self.config.caldav_url, body, &[("Depth", "0")])
            .await?;

        // Parse ctag from XML response
        extract_xml_value(&resp, "getctag")
            .ok_or_else(|| anyhow::anyhow!("no ctag in CalDAV response"))
    }

    /// List all events in the calendar with their hrefs and etags.
    async fn list_events(&self) -> anyhow::Result<Vec<CalDavEvent>> {
        let now = Utc::now();
        let start = now - chrono::Duration::days(1);
        let end = now + chrono::Duration::days(14);

        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:getetag/>
    <c:calendar-data/>
  </d:prop>
  <c:filter>
    <c:comp-filter name="VCALENDAR">
      <c:comp-filter name="VEVENT">
        <c:time-range start="{}" end="{}"/>
      </c:comp-filter>
    </c:comp-filter>
  </c:filter>
</c:calendar-query>"#,
            start.format("%Y%m%dT%H%M%SZ"),
            end.format("%Y%m%dT%H%M%SZ"),
        );

        let resp = self
            .request("REPORT", &self.config.caldav_url, &body, &[("Depth", "1")])
            .await?;

        parse_calendar_response(&resp)
    }

    /// Make an authenticated request to the CalDAV server.
    async fn request(
        &self,
        method: &str,
        url: &str,
        body: &str,
        headers: &[(&str, &str)],
    ) -> anyhow::Result<String> {
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|e| anyhow::anyhow!("invalid HTTP method: {e}"))?;

        let mut req = self.http.request(method, url)
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(body.to_string());

        for (k, v) in headers {
            req = req.header(*k, *v);
        }

        // Add Basic auth from config or env
        let username = self.config.username.clone()
            .or_else(|| std::env::var("SENTINEL_CALDAV_USER").ok());
        let password = self.config.password.clone()
            .or_else(|| std::env::var("SENTINEL_CALDAV_PASS").ok());
        if let (Some(user), Some(pass)) = (username, password) {
            req = req.basic_auth(user, Some(pass));
        }

        let resp = req.send().await
            .map_err(|e| anyhow::anyhow!("CalDAV request failed: {e}"))?;

        if !resp.status().is_success() && resp.status().as_u16() != 207 {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("CalDAV returned HTTP {status}: {body}");
        }

        resp.text().await.map_err(|e| anyhow::anyhow!("CalDAV response read failed: {e}"))
    }
}

/// Extract a simple XML element value by tag name (no namespace prefix).
fn extract_xml_value(xml: &str, tag: &str) -> Option<String> {
    // Simple parser — CalDAV responses are well-structured
    let patterns = [
        format!("<{tag}>"),
        format!("<d:{tag}>"),
        format!("<D:{tag}>"),
        format!("<cs:{tag}>"),
        format!("<ns0:{tag}>"),
        format!("<C:{tag}>"),
        format!("<c:{tag}>"),
    ];
    for prefix_pat in &patterns {
        if let Some(start) = xml.find(prefix_pat.as_str()) {
            let value_start = start + prefix_pat.len();
            if let Some(end) = xml[value_start..].find('<') {
                return Some(xml[value_start..value_start + end].trim().to_string());
            }
        }
    }
    None
}

/// Parse a CalDAV REPORT response into events.
fn parse_calendar_response(xml: &str) -> anyhow::Result<Vec<CalDavEvent>> {
    let mut events = Vec::new();

    // Split by <d:response> or <response> blocks
    let response_tag_variants = ["<d:response>", "<D:response>", "<response>"];
    let end_variants = ["</d:response>", "</D:response>", "</response>"];

    let mut remaining = xml;
    loop {
        // Find next response block
        let start = response_tag_variants
            .iter()
            .filter_map(|tag| remaining.find(tag).map(|pos| (pos, tag.len())))
            .min_by_key(|(pos, _)| *pos);

        let Some((start_pos, tag_len)) = start else {
            break;
        };

        remaining = &remaining[start_pos + tag_len..];

        let end = end_variants
            .iter()
            .filter_map(|tag| remaining.find(tag).map(|pos| (pos, tag.len())))
            .min_by_key(|(pos, _)| *pos);

        let Some((end_pos, _)) = end else { break };

        let block = &remaining[..end_pos];
        remaining = &remaining[end_pos..];

        // Extract href
        let href = extract_xml_value(block, "href")
            .unwrap_or_default();
        if href.is_empty() { continue; }

        // Extract etag
        let etag = extract_xml_value(block, "getetag")
            .unwrap_or_default();

        // Extract calendar-data (iCalendar format)
        let cal_data = extract_xml_value(block, "calendar-data")
            .unwrap_or_default();
        if cal_data.is_empty() { continue; }

        // Parse iCalendar data
        if let Some(event) = parse_icalendar(&cal_data) {
            events.push(CalDavEvent { href, etag, event });
        }
    }

    Ok(events)
}

/// Unfold iCalendar long lines per RFC 5545 §3.1.
///
/// Lines longer than 75 octets are folded by inserting a CRLF followed by
/// a single whitespace character (space or tab). This function reverses that.
fn unfold_ical(ical: &str) -> String {
    // Replace CRLF + whitespace continuation, and also bare LF + whitespace
    let mut result = String::with_capacity(ical.len());
    let mut chars = ical.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next(); // consume \n
                if chars.peek() == Some(&' ') || chars.peek() == Some(&'\t') {
                    chars.next(); // consume continuation whitespace — join lines
                } else {
                    result.push('\n'); // real line break
                }
            } else {
                result.push(c);
            }
        } else if c == '\n' {
            if chars.peek() == Some(&' ') || chars.peek() == Some(&'\t') {
                chars.next(); // consume continuation whitespace — join lines
            } else {
                result.push(c);
            }
        } else {
            result.push(c);
        }
    }
    result
}

/// Parse a minimal VEVENT from iCalendar data.
fn parse_icalendar(ical: &str) -> Option<CalendarEvent> {
    let unfolded = unfold_ical(ical);
    let mut title = None;
    let mut start: Option<DateTime<Utc>> = None;
    let mut end: Option<DateTime<Utc>> = None;
    let mut location = None;
    let mut description = None;
    let mut all_day = false;

    for line in unfolded.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("SUMMARY:") {
            title = Some(val.to_string());
        } else if let Some(val) = line.strip_prefix("DTSTART:") {
            start = parse_ical_datetime(val);
        } else if let Some(val) = line.strip_prefix("DTSTART;VALUE=DATE:") {
            start = parse_ical_date(val);
            all_day = true;
        } else if line.starts_with("DTSTART;") {
            // DTSTART;TZID=Europe/Lisbon:20260221T090000
            if let Some(colon_pos) = line.find(':') {
                start = parse_ical_datetime(&line[colon_pos + 1..]);
            }
        } else if let Some(val) = line.strip_prefix("DTEND:") {
            end = parse_ical_datetime(val);
        } else if let Some(val) = line.strip_prefix("DTEND;VALUE=DATE:") {
            end = parse_ical_date(val);
        } else if line.starts_with("DTEND;") {
            if let Some(colon_pos) = line.find(':') {
                end = parse_ical_datetime(&line[colon_pos + 1..]);
            }
        } else if let Some(val) = line.strip_prefix("LOCATION:") {
            location = Some(val.to_string());
        } else if let Some(val) = line.strip_prefix("DESCRIPTION:") {
            description = Some(val.to_string());
        }
    }

    let title = title?;
    let start = start?;

    Some(CalendarEvent {
        title,
        start,
        end,
        location,
        description,
        all_day,
    })
}

/// Parse an iCalendar datetime (20260221T090000Z or 20260221T090000).
fn parse_ical_datetime(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    // Try UTC format: 20260221T090000Z
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s.trim_end_matches('Z'), "%Y%m%dT%H%M%S") {
        return Some(dt.and_utc());
    }
    None
}

/// Parse an iCalendar date (20260221) as midnight UTC.
fn parse_ical_date(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y%m%d") {
        return Some(d.and_hms_opt(0, 0, 0)?.and_utc());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_vevent() {
        let ical = "BEGIN:VCALENDAR\r\n\
            BEGIN:VEVENT\r\n\
            SUMMARY:Dentist\r\n\
            DTSTART:20260221T140000Z\r\n\
            DTEND:20260221T150000Z\r\n\
            LOCATION:Clínica São João\r\n\
            END:VEVENT\r\n\
            END:VCALENDAR";
        let event = parse_icalendar(ical).unwrap();
        assert_eq!(event.title, "Dentist");
        assert_eq!(event.location.as_deref(), Some("Clínica São João"));
        assert!(!event.all_day);
    }

    #[test]
    fn parse_all_day_event() {
        let ical = "BEGIN:VEVENT\r\n\
            SUMMARY:Holiday\r\n\
            DTSTART;VALUE=DATE:20260225\r\n\
            DTEND;VALUE=DATE:20260226\r\n\
            END:VEVENT";
        let event = parse_icalendar(ical).unwrap();
        assert_eq!(event.title, "Holiday");
        assert!(event.all_day);
    }

    #[test]
    fn parse_tzid_datetime() {
        let ical = "BEGIN:VEVENT\r\n\
            SUMMARY:Standup\r\n\
            DTSTART;TZID=Europe/Lisbon:20260221T090000\r\n\
            END:VEVENT";
        let event = parse_icalendar(ical).unwrap();
        assert_eq!(event.title, "Standup");
        assert!(event.start.timestamp() > 0);
    }

    #[test]
    fn extract_ctag_from_propfind() {
        let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:cs="http://calendarserver.org/ns/">
  <d:response>
    <d:propstat>
      <d:prop>
        <cs:getctag>abc123</cs:getctag>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#;
        assert_eq!(extract_xml_value(xml, "getctag"), Some("abc123".into()));
    }

    #[test]
    fn parse_multistatus_response() {
        let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/cal/event1.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"etag1"</d:getetag>
        <c:calendar-data>BEGIN:VCALENDAR
BEGIN:VEVENT
SUMMARY:Team Meeting
DTSTART:20260221T100000Z
DTEND:20260221T110000Z
LOCATION:Room 3
END:VEVENT
END:VCALENDAR</c:calendar-data>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#;
        let events = parse_calendar_response(xml).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].href, "/cal/event1.ics");
        assert_eq!(events[0].event.title, "Team Meeting");
        assert_eq!(events[0].event.location.as_deref(), Some("Room 3"));
    }

    #[test]
    fn rejects_missing_summary() {
        let ical = "BEGIN:VEVENT\r\n\
            DTSTART:20260221T090000Z\r\n\
            END:VEVENT";
        assert!(parse_icalendar(ical).is_none());
    }

    #[test]
    fn rejects_missing_dtstart() {
        let ical = "BEGIN:VEVENT\r\n\
            SUMMARY:Orphan\r\n\
            END:VEVENT";
        assert!(parse_icalendar(ical).is_none());
    }

    #[test]
    fn unfold_long_lines() {
        // RFC 5545: long lines are folded with CRLF + space.
        // The space/tab after CRLF is the fold indicator and gets removed;
        // any space BEFORE the fold is part of the original content.
        let ical = "BEGIN:VEVENT\r\n\
            SUMMARY:Very long meeting title that got \r\n folded by the server\r\n\
            DTSTART:20260221T090000Z\r\n\
            LOCATION:Conference Room Building A Floor\r\n  3 West Wing\r\n\
            END:VEVENT";
        let event = parse_icalendar(ical).unwrap();
        assert_eq!(
            event.title,
            "Very long meeting title that got folded by the server"
        );
        assert_eq!(
            event.location.as_deref(),
            Some("Conference Room Building A Floor 3 West Wing")
        );
    }
}
