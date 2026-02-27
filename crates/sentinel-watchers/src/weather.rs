use sentinel_core::config::WeatherConfig;
use sentinel_core::events::{WatchEvent, WeatherUpdate};

/// Weather data fetcher using Open-Meteo (free, no API key).
///
/// Polls the Open-Meteo forecast API periodically and emits
/// WeatherUpdate events. Only emits when data is successfully fetched.
pub struct WeatherWatcher {
    config: WeatherConfig,
    http: reqwest::Client,
}

/// Open-Meteo current weather response.
#[derive(Debug, serde::Deserialize)]
struct OpenMeteoResponse {
    current: Option<CurrentWeather>,
    daily: Option<DailyForecast>,
}

#[derive(Debug, serde::Deserialize)]
struct CurrentWeather {
    temperature_2m: Option<f64>,
    #[serde(default)]
    weather_code: Option<i32>,
}

#[derive(Debug, serde::Deserialize)]
struct DailyForecast {
    #[serde(default)]
    weather_code: Vec<i32>,
    #[serde(default)]
    temperature_2m_max: Vec<f64>,
    #[serde(default)]
    temperature_2m_min: Vec<f64>,
}

impl WeatherWatcher {
    pub fn new(config: WeatherConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("failed to build HTTP client");
        Self { config, http }
    }

    /// Run the watcher loop. Polls Open-Meteo and sends weather updates.
    pub async fn run(&self, tx: tokio::sync::mpsc::Sender<WatchEvent>) -> anyhow::Result<()> {
        tracing::info!(
            lat = self.config.lat,
            lon = self.config.lon,
            "weather watcher starting"
        );

        let poll_interval =
            std::time::Duration::from_secs(self.config.poll_interval_secs);

        loop {
            match self.fetch_weather().await {
                Ok(update) => {
                    tracing::debug!(
                        temp = update.temperature_c,
                        conditions = %update.conditions,
                        "weather fetched"
                    );
                    if tx.send(WatchEvent::Weather(update)).await.is_err() {
                        tracing::info!("event channel closed, weather watcher stopping");
                        return Ok(());
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "weather fetch failed, retrying next cycle");
                }
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Fetch current weather and 3-day forecast from Open-Meteo.
    pub async fn fetch_weather(&self) -> anyhow::Result<WeatherUpdate> {
        let url = format!(
            "https://api.open-meteo.com/v1/forecast\
             ?latitude={}&longitude={}\
             &current=temperature_2m,weather_code\
             &daily=weather_code,temperature_2m_max,temperature_2m_min\
             &timezone=auto&forecast_days=3",
            self.config.lat, self.config.lon,
        );

        let resp: OpenMeteoResponse = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Open-Meteo request failed: {e}"))?
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("Open-Meteo response parse failed: {e}"))?;

        let current = resp
            .current
            .ok_or_else(|| anyhow::anyhow!("no current weather in response"))?;

        let temperature_c = current.temperature_2m.unwrap_or(0.0);
        let conditions = wmo_code_to_description(current.weather_code.unwrap_or(0));

        let forecast = resp
            .daily
            .map(|d| {
                d.weather_code
                    .iter()
                    .zip(d.temperature_2m_max.iter().zip(d.temperature_2m_min.iter()))
                    .map(|(code, (max, min))| {
                        format!(
                            "{}: {:.0}°C/{:.0}°C",
                            wmo_code_to_description(*code),
                            max,
                            min,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(WeatherUpdate {
            location: format!("{:.2},{:.2}", self.config.lat, self.config.lon),
            temperature_c,
            conditions,
            forecast,
        })
    }
}

/// Convert WMO weather interpretation code to human-readable description.
fn wmo_code_to_description(code: i32) -> String {
    match code {
        0 => "Clear sky".into(),
        1 => "Mainly clear".into(),
        2 => "Partly cloudy".into(),
        3 => "Overcast".into(),
        45 | 48 => "Foggy".into(),
        51 => "Light drizzle".into(),
        53 => "Moderate drizzle".into(),
        55 => "Dense drizzle".into(),
        61 => "Slight rain".into(),
        63 => "Moderate rain".into(),
        65 => "Heavy rain".into(),
        66 | 67 => "Freezing rain".into(),
        71 => "Slight snow".into(),
        73 => "Moderate snow".into(),
        75 => "Heavy snow".into(),
        77 => "Snow grains".into(),
        80 => "Slight rain showers".into(),
        81 => "Moderate rain showers".into(),
        82 => "Violent rain showers".into(),
        85 | 86 => "Snow showers".into(),
        95 => "Thunderstorm".into(),
        96 | 99 => "Thunderstorm with hail".into(),
        _ => format!("Weather code {code}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wmo_clear() {
        assert_eq!(wmo_code_to_description(0), "Clear sky");
    }

    #[test]
    fn wmo_rain() {
        assert_eq!(wmo_code_to_description(63), "Moderate rain");
    }

    #[test]
    fn wmo_thunderstorm() {
        let desc = wmo_code_to_description(95);
        assert!(desc.contains("Thunderstorm"));
    }

    #[test]
    fn wmo_unknown() {
        let desc = wmo_code_to_description(999);
        assert!(desc.contains("999"));
    }

    #[test]
    fn weather_update_format() {
        let update = WeatherUpdate {
            location: "41.15,-8.61".into(),
            temperature_c: 14.5,
            conditions: "Partly cloudy".into(),
            forecast: vec![
                "Slight rain: 16°C/10°C".into(),
                "Overcast: 14°C/9°C".into(),
            ],
        };
        assert!(update.conditions.contains("cloudy"));
        assert_eq!(update.forecast.len(), 2);
    }
}
