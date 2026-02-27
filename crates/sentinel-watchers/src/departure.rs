use chrono::{DateTime, Utc};
use sentinel_core::config::{DepartureConfig, RoutingConfig};
use sentinel_core::events::{DepartureEvent, WatchEvent};

/// Departure watcher with routing and weather-aware alerts.
///
/// Periodically scans upcoming calendar events (from the CalDAV cache in the
/// event channel) for events with locations, computes travel time via OSRM
/// or TomTom, and emits a DepartureEvent when it's time to leave.
///
/// Key design: this watcher doesn't talk to CalDAV directly — it receives
/// calendar events via a shared cache that the CalDAV watcher maintains.
/// This avoids duplicate CalDAV polling.
pub struct DepartureWatcher {
    departure_config: DepartureConfig,
    routing_config: RoutingConfig,
    http: reqwest::Client,
    /// Events we've already alerted about (to avoid duplicate alerts).
    alerted: std::sync::Arc<tokio::sync::Mutex<std::collections::HashSet<String>>>,
    /// Upcoming calendar events with locations (fed from CalDAV watcher).
    upcoming: std::sync::Arc<tokio::sync::Mutex<Vec<UpcomingEvent>>>,
    /// Current weather conditions (fed from weather watcher via daemon).
    weather_conditions: std::sync::Arc<tokio::sync::Mutex<Option<String>>>,
}

/// A calendar event with a location that may trigger a departure alert.
#[derive(Debug, Clone)]
pub struct UpcomingEvent {
    pub title: String,
    pub start: DateTime<Utc>,
    pub location: String,
    pub event_id: String,
}

/// Routing result from OSRM or TomTom.
#[derive(Debug, Clone)]
pub struct RouteResult {
    pub duration_minutes: u32,
    pub distance_km: f64,
}

/// OSRM route response.
#[derive(Debug, serde::Deserialize)]
struct OsrmResponse {
    #[serde(default)]
    routes: Vec<OsrmRoute>,
    code: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct OsrmRoute {
    duration: f64, // seconds
    distance: f64, // meters
}

impl DepartureWatcher {
    pub fn new(departure_config: DepartureConfig, routing_config: RoutingConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("failed to build HTTP client");
        Self {
            departure_config,
            routing_config,
            http,
            alerted: std::sync::Arc::new(tokio::sync::Mutex::new(
                std::collections::HashSet::new(),
            )),
            upcoming: std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new())),
            weather_conditions: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    /// Get a handle to update upcoming events (called by daemon when CalDAV watcher reports changes).
    pub fn upcoming_events(&self) -> std::sync::Arc<tokio::sync::Mutex<Vec<UpcomingEvent>>> {
        self.upcoming.clone()
    }

    /// Get a handle to update weather conditions (called by daemon when weather watcher reports).
    pub fn weather_handle(&self) -> std::sync::Arc<tokio::sync::Mutex<Option<String>>> {
        self.weather_conditions.clone()
    }

    /// Run the watcher loop. Checks upcoming events for departure timing.
    pub async fn run(&self, tx: tokio::sync::mpsc::Sender<WatchEvent>) -> anyhow::Result<()> {
        tracing::info!("departure watcher starting");

        let check_interval =
            std::time::Duration::from_secs(self.departure_config.check_interval_secs);
        let lookahead = chrono::Duration::hours(self.departure_config.lookahead_hours as i64);

        loop {
            let now = Utc::now();
            let horizon = now + lookahead;

            // Get events within the lookahead window that have locations
            let events: Vec<UpcomingEvent> = {
                let upcoming = self.upcoming.lock().await;
                upcoming
                    .iter()
                    .filter(|e| e.start > now && e.start <= horizon)
                    .cloned()
                    .collect()
            };

            for event in events {
                // Skip if already alerted
                {
                    let alerted = self.alerted.lock().await;
                    if alerted.contains(&event.event_id) {
                        continue;
                    }
                }

                // Try to get routing info
                match self.get_route(&event.location).await {
                    Ok(route) => {
                        let weather_buf = {
                            let cond = self.weather_conditions.lock().await;
                            weather_buffer_minutes(cond.as_deref())
                        };
                        let buffer = self.departure_config.comfort_buffer_minutes + weather_buf;
                        let total_minutes = route.duration_minutes + buffer;
                        let leave_by = event.start
                            - chrono::Duration::minutes(total_minutes as i64);

                        // Alert if we should leave within the next check interval + buffer
                        let alert_window = now
                            + chrono::Duration::seconds(
                                self.departure_config.check_interval_secs as i64,
                            );

                        if leave_by <= alert_window {
                            tracing::info!(
                                destination = %event.title,
                                travel_min = route.duration_minutes,
                                leave_by = %leave_by,
                                "departure alert triggered"
                            );

                            let departure = DepartureEvent {
                                destination: format!(
                                    "{} — {}",
                                    event.title, event.location
                                ),
                                event_time: event.start,
                                travel_minutes: route.duration_minutes,
                                leave_by,
                            };

                            // Mark as alerted
                            self.alerted.lock().await.insert(event.event_id.clone());

                            if tx
                                .send(WatchEvent::Departure(departure))
                                .await
                                .is_err()
                            {
                                tracing::info!(
                                    "event channel closed, departure watcher stopping"
                                );
                                return Ok(());
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            destination = %event.location,
                            error = %e,
                            "routing query failed"
                        );
                    }
                }
            }

            // Clean expired alerts (events already past)
            {
                let now = Utc::now();
                let upcoming = self.upcoming.lock().await;
                let current_ids: std::collections::HashSet<String> =
                    upcoming.iter().map(|e| e.event_id.clone()).collect();
                self.alerted
                    .lock()
                    .await
                    .retain(|id| current_ids.contains(id));
                // Also clean past events from upcoming
                drop(upcoming);
                let mut upcoming = self.upcoming.lock().await;
                upcoming.retain(|e| e.start > now);
            }

            tokio::time::sleep(check_interval).await;
        }
    }

    /// Get route from home to destination via OSRM.
    async fn get_route(&self, destination: &str) -> anyhow::Result<RouteResult> {
        match self.routing_config.provider.as_str() {
            "osrm" => self.get_route_osrm(destination).await,
            other => anyhow::bail!("unsupported routing provider: {other}"),
        }
    }

    /// Query OSRM for route duration.
    ///
    /// OSRM expects coordinates, not addresses. For now we accept
    /// "lat,lon" format in event locations. A geocoding step can be
    /// added later.
    async fn get_route_osrm(&self, destination: &str) -> anyhow::Result<RouteResult> {
        let (dest_lon, dest_lat) = parse_coordinates(destination)
            .ok_or_else(|| anyhow::anyhow!(
                "cannot parse coordinates from location: {destination}. \
                 Expected 'lat,lon' format or geocoded coordinates."
            ))?;

        let url = format!(
            "{}/route/v1/driving/{},{};{},{}?overview=false",
            self.routing_config.endpoint.trim_end_matches('/'),
            self.departure_config.home_lon,
            self.departure_config.home_lat,
            dest_lon,
            dest_lat,
        );

        let resp: OsrmResponse = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("OSRM request failed: {e}"))?
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("OSRM response parse failed: {e}"))?;

        if resp.code.as_deref() != Some("Ok") {
            anyhow::bail!("OSRM error: {:?}", resp.code);
        }

        let route = resp
            .routes
            .first()
            .ok_or_else(|| anyhow::anyhow!("OSRM returned no routes"))?;

        Ok(RouteResult {
            duration_minutes: (route.duration / 60.0).ceil() as u32,
            distance_km: route.distance / 1000.0,
        })
    }
}

/// Parse "lat,lon" or similar coordinate formats from a location string.
/// Returns (lon, lat) for OSRM format.
fn parse_coordinates(location: &str) -> Option<(f64, f64)> {
    // Try "lat,lon" format (most common in calendar events)
    let parts: Vec<&str> = location.split(',').collect();
    if parts.len() == 2 {
        let lat = parts[0].trim().parse::<f64>().ok()?;
        let lon = parts[1].trim().parse::<f64>().ok()?;
        return Some((lon, lat)); // OSRM wants lon,lat
    }
    None
}

/// Extra buffer minutes based on weather conditions.
/// Rain adds 5 min, snow/storm/freezing adds 10 min.
fn weather_buffer_minutes(conditions: Option<&str>) -> u32 {
    let Some(c) = conditions else { return 0 };
    let lower = c.to_lowercase();
    if lower.contains("snow")
        || lower.contains("storm")
        || lower.contains("freezing")
        || lower.contains("thunder")
        || lower.contains("blizzard")
        || lower.contains("ice")
    {
        10
    } else if lower.contains("rain")
        || lower.contains("drizzle")
        || lower.contains("shower")
        || lower.contains("sleet")
    {
        5
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_lat_lon_coordinates() {
        let (lon, lat) = parse_coordinates("41.1579, -8.6291").unwrap();
        assert!((lat - 41.1579).abs() < 0.001);
        assert!((lon - (-8.6291)).abs() < 0.001);
    }

    #[test]
    fn parse_coordinates_rejects_text() {
        assert!(parse_coordinates("Clínica São João, Matosinhos").is_none());
    }

    #[test]
    fn departure_event_format() {
        let dep = DepartureEvent {
            destination: "Dentist — Clínica São João".into(),
            event_time: Utc::now() + chrono::Duration::hours(2),
            travel_minutes: 18,
            leave_by: Utc::now() + chrono::Duration::hours(1),
        };
        assert!(dep.destination.contains("Dentist"));
        assert_eq!(dep.travel_minutes, 18);
    }

    #[test]
    fn route_result_structure() {
        let route = RouteResult {
            duration_minutes: 25,
            distance_km: 18.5,
        };
        assert_eq!(route.duration_minutes, 25);
        assert!((route.distance_km - 18.5).abs() < 0.1);
    }

    #[test]
    fn upcoming_event_structure() {
        let event = UpcomingEvent {
            title: "Dentist".into(),
            start: Utc::now() + chrono::Duration::hours(3),
            location: "41.1579,-8.6291".into(),
            event_id: "event-123".into(),
        };
        assert_eq!(event.title, "Dentist");
    }

    #[test]
    fn weather_buffer_rain() {
        assert_eq!(weather_buffer_minutes(Some("Light rain")), 5);
        assert_eq!(weather_buffer_minutes(Some("Moderate drizzle")), 5);
        assert_eq!(weather_buffer_minutes(Some("Rain showers")), 5);
    }

    #[test]
    fn weather_buffer_severe() {
        assert_eq!(weather_buffer_minutes(Some("Thunderstorm")), 10);
        assert_eq!(weather_buffer_minutes(Some("Heavy snow")), 10);
        assert_eq!(weather_buffer_minutes(Some("Freezing rain")), 10);
    }

    #[test]
    fn weather_buffer_clear() {
        assert_eq!(weather_buffer_minutes(Some("Clear sky")), 0);
        assert_eq!(weather_buffer_minutes(Some("Partly cloudy")), 0);
        assert_eq!(weather_buffer_minutes(None), 0);
    }
}
