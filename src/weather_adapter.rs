use crate::weather::{
    RawForecast, WeatherSource, filter_to_window, forecast_hourly_url, parse_active_alerts,
    parse_periods,
};
use chrono::{DateTime, Utc};

const USER_AGENT: &str = "hike-club-api (contact: scondon87@gmail.com)";
/// ponytail: Cache API only, no KV. Add KV if cross-colo cache sharing matters.
const CACHE_TTL_SECS: u32 = 600;

pub struct NwsWeatherSource;

impl WeatherSource for NwsWeatherSource {
    async fn forecast(
        &self,
        lat: f64,
        lon: f64,
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
    ) -> Result<RawForecast, String> {
        let cache_key = format!(
            "https://cache.internal/weather?lat={:.2}&lon={:.2}",
            lat, lon
        );
        let cache = worker::Cache::default();
        let cache_request =
            worker::Request::new(&cache_key, worker::Method::Get).map_err(|e| e.to_string())?;

        if let Some(mut cached) = cache
            .get(&cache_request, false)
            .await
            .map_err(|e| e.to_string())?
        {
            let body = cached.text().await.map_err(|e| e.to_string())?;
            if let Ok(raw) = serde_json::from_str::<RawForecast>(&body) {
                return Ok(filter_to_window(raw, window_start, window_end));
            }
        }

        let raw = fetch_nws_forecast(lat, lon).await?;

        let body = serde_json::to_string(&raw).map_err(|e| e.to_string())?;
        let headers = worker::Headers::new();
        headers
            .set("cache-control", &format!("max-age={CACHE_TTL_SECS}"))
            .map_err(|e| e.to_string())?;
        let response = worker::Response::ok(body)
            .map_err(|e| e.to_string())?
            .with_headers(headers);
        cache
            .put(&cache_request, response)
            .await
            .map_err(|e| e.to_string())?;

        Ok(filter_to_window(raw, window_start, window_end))
    }
}

async fn fetch_nws_forecast(lat: f64, lon: f64) -> Result<RawForecast, String> {
    let points_url = format!("https://api.weather.gov/points/{lat:.4},{lon:.4}");
    let points: serde_json::Value = get_json(&points_url).await?;
    let forecast_hourly_url = forecast_hourly_url(&points)?;

    let forecast: serde_json::Value = get_json(&forecast_hourly_url).await?;
    let periods = parse_periods(&forecast);

    let alerts_url = format!("https://api.weather.gov/alerts/active?point={lat:.4},{lon:.4}");
    let alerts: serde_json::Value = get_json(&alerts_url).await?;
    let active_alerts = parse_active_alerts(&alerts);

    Ok(RawForecast {
        periods,
        active_alerts,
    })
}

async fn get_json(url: &str) -> Result<serde_json::Value, String> {
    let headers = worker::Headers::new();
    headers
        .set("User-Agent", USER_AGENT)
        .map_err(|e| e.to_string())?;
    let request = worker::Request::new_with_init(
        url,
        &worker::RequestInit {
            method: worker::Method::Get,
            headers,
            ..Default::default()
        },
    )
    .map_err(|e| e.to_string())?;
    let mut response = worker::Fetch::Request(request)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    response.json().await.map_err(|e| e.to_string())
}
