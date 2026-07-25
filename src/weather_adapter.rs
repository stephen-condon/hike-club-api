use crate::weather::{
    RawForecast, WeatherSource, first_station_url, forecast_hourly_url, nws_query_time,
    observation_stations_url, parse_active_alerts, parse_observations, parse_periods,
};
use chrono::{DateTime, Utc};

const USER_AGENT: &str = "hike-club-api (contact: scondon87@gmail.com)";
/// ponytail: Cache API only, no KV. Add KV if cross-colo cache sharing matters.
const FORECAST_TTL_SECS: u32 = 600;
/// Past observations never change, so cache them for a day.
const OBSERVATION_TTL_SECS: u32 = 86_400;

pub struct NwsWeatherSource;

impl WeatherSource for NwsWeatherSource {
    async fn forecast(
        &self,
        lat: f64,
        lon: f64,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<RawForecast, String> {
        // A fully-past hike has no forecast coverage (NWS hourly forecast is
        // future-only), so pull the actual observed weather instead.
        let now = DateTime::<Utc>::from_timestamp_millis(worker::Date::now().as_millis() as i64)
            .ok_or_else(|| "invalid current time".to_string())?;

        if end < now {
            let key = format!(
                "https://cache.internal/observed?lat={lat:.2}&lon={lon:.2}&day={}",
                start.date_naive()
            );
            cached(
                &key,
                OBSERVATION_TTL_SECS,
                fetch_nws_observations(lat, lon, start, end),
            )
            .await
        } else {
            let key = format!("https://cache.internal/weather?lat={lat:.2}&lon={lon:.2}");
            cached(&key, FORECAST_TTL_SECS, fetch_nws_forecast(lat, lon)).await
        }
    }
}

/// Cache-API read-through: serve a cached `RawForecast` for `key`, else run
/// `fetch`, cache it under `ttl`, and return it.
async fn cached(
    key: &str,
    ttl: u32,
    fetch: impl Future<Output = Result<RawForecast, String>>,
) -> Result<RawForecast, String> {
    let cache = worker::Cache::default();
    let cache_request =
        worker::Request::new(key, worker::Method::Get).map_err(|e| e.to_string())?;

    if let Some(mut hit) = cache
        .get(&cache_request, false)
        .await
        .map_err(|e| e.to_string())?
    {
        let body = hit.text().await.map_err(|e| e.to_string())?;
        if let Ok(raw) = serde_json::from_str::<RawForecast>(&body) {
            return Ok(raw);
        }
    }

    let raw = fetch.await?;

    let body = serde_json::to_string(&raw).map_err(|e| e.to_string())?;
    let headers = worker::Headers::new();
    headers
        .set("cache-control", &format!("max-age={ttl}"))
        .map_err(|e| e.to_string())?;
    let response = worker::Response::ok(body)
        .map_err(|e| e.to_string())?
        .with_headers(headers);
    cache
        .put(&cache_request, response)
        .await
        .map_err(|e| e.to_string())?;

    Ok(raw)
}

/// Fetches actual observed weather for a completed hike from the nearest NWS
/// station. ponytail: no historical NWS watch/warning alerts here — active alerts
/// are a *now* concept; add a `/alerts?start=&end=` fetch if past alerts matter.
async fn fetch_nws_observations(
    lat: f64,
    lon: f64,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<RawForecast, String> {
    let points_url = format!("https://api.weather.gov/points/{lat:.4},{lon:.4}");
    let points: serde_json::Value = get_json(&points_url).await?;
    let stations_url = observation_stations_url(&points)?;

    let stations: serde_json::Value = get_json(&stations_url).await?;
    let station_url = first_station_url(&stations)?;

    let obs_url = format!(
        "{station_url}/observations?start={}&end={}",
        nws_query_time(start),
        nws_query_time(end)
    );
    let obs: serde_json::Value = get_json(&obs_url).await?;

    Ok(RawForecast {
        periods: parse_observations(&obs),
        alerts: vec![],
    })
}

async fn fetch_nws_forecast(lat: f64, lon: f64) -> Result<RawForecast, String> {
    let points_url = format!("https://api.weather.gov/points/{lat:.4},{lon:.4}");
    let points: serde_json::Value = get_json(&points_url).await?;
    let forecast_hourly_url = forecast_hourly_url(&points)?;

    let forecast: serde_json::Value = get_json(&forecast_hourly_url).await?;
    let periods = parse_periods(&forecast);

    let alerts_url = format!("https://api.weather.gov/alerts/active?point={lat:.4},{lon:.4}");
    let alerts_json: serde_json::Value = get_json(&alerts_url).await?;
    let alerts = parse_active_alerts(&alerts_json);

    Ok(RawForecast { periods, alerts })
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
