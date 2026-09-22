use crate::weather::{
    RawForecast, WeatherSource, forecast_cache_key, forecast_hourly_url, nws_query_time,
    observation_cache_key, observation_stations_url, parse_active_alerts, parse_observations,
    parse_periods, station_urls,
};
use chrono::{DateTime, FixedOffset, Utc};

const USER_AGENT: &str = "hike-club-api (contact: scondon87@gmail.com)";
/// ponytail: Cache API only, no KV. Add KV if cross-colo cache sharing matters.
const FORECAST_TTL_SECS: u32 = 600;
/// Past observations never change, so cache them for a day.
const OBSERVATION_TTL_SECS: u32 = 86_400;

pub struct NwsWeatherSource;

// @spec WX-SRC-001, WX-SRC-002
impl WeatherSource for NwsWeatherSource {
    async fn forecast(
        &self,
        lat: f64,
        lon: f64,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        offset: FixedOffset,
    ) -> Result<RawForecast, String> {
        // A fully-past hike has no forecast coverage (NWS hourly forecast is
        // future-only), so pull the actual observed weather instead.
        let now = DateTime::<Utc>::from_timestamp_millis(worker::Date::now().as_millis() as i64)
            .ok_or_else(|| "invalid current time".to_string())?;

        if end < now {
            let key = observation_cache_key(lat, lon, start, offset);
            // A completed hike's absent readings never appear later, so an
            // exhausted station walk (WX-SRC-008) is cached as a settled
            // answer rather than an upstream gap.
            cached(
                &key,
                OBSERVATION_TTL_SECS,
                true,
                fetch_nws_observations(lat, lon, start, end),
            )
            .await
        } else {
            let key = forecast_cache_key(lat, lon);
            // An empty forecast is an upstream gap, not authoritative "no
            // weather", so it is never cached.
            cached(&key, FORECAST_TTL_SECS, false, fetch_nws_forecast(lat, lon)).await
        }
    }
}

/// Cache-API read-through: serve a cached `RawForecast` for `key`, else run
/// `fetch`, cache it under `ttl`, and return it. `cache_empty` marks a
/// periodless result as a settled answer rather than an upstream gap — true
/// for observations (a completed hike's absent readings will not appear
/// later, once WX-SRC-008 has exhausted every permitted station), false for
/// forecasts (an empty one is always a gap).
// @spec WX-CACHE-001, WX-CACHE-004, WX-CACHE-005, WX-CACHE-006, WX-CACHE-007,
// @spec WX-CACHE-008, WX-CACHE-009
async fn cached(
    key: &str,
    ttl: u32,
    cache_empty: bool,
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
            // An empty cached entry is a non-answer (past upstream gap/error)
            // unless `cache_empty` marks it a settled one — otherwise treat it
            // as a miss and refetch, which also self-heals any such entry
            // cached before this guard existed.
            if !raw.periods.is_empty() || cache_empty {
                return Ok(raw);
            }
        }
    }

    let raw = fetch.await?;

    // Don't cache a periodless result unless the caller says it's settled: an
    // upstream gap would otherwise suppress weather for the whole TTL; instead
    // let the next request retry.
    if raw.periods.is_empty() && !cache_empty {
        return Ok(raw);
    }

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

/// Fetches actual observed weather for a completed hike, trying up to three
/// nearest NWS stations in proximity order and stopping at the first that
/// yields readings for the window (WX-SRC-008) — a listed station is not
/// necessarily a reporting one. Exhausting all three returns an empty result.
/// ponytail: no historical NWS watch/warning alerts here — active alerts
/// are a *now* concept; add a `/alerts?start=&end=` fetch if past alerts matter.
// @spec WX-SRC-004, WX-SRC-007, WX-SRC-008, WX-SRC-009, WX-ALERT-012
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

    for station_url in station_urls(&stations)? {
        let obs_url = format!(
            "{station_url}/observations?start={}&end={}",
            nws_query_time(start),
            nws_query_time(end)
        );
        let obs: serde_json::Value = get_json(&obs_url).await?;
        let periods = parse_observations(&obs);
        if !periods.is_empty() {
            return Ok(RawForecast {
                periods,
                alerts: vec![],
            });
        }
    }

    Ok(RawForecast {
        periods: vec![],
        alerts: vec![],
    })
}

// @spec WX-SRC-004, WX-SRC-006, WX-CACHE-007
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

// @spec WX-SRC-003, WX-SRC-005, WX-SRC-011
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
    // NWS error responses (e.g. a 400) still carry a JSON body. Without this
    // check we'd parse that error body as data — an empty forecast — and cache
    // it as if it were real, so a transient upstream failure would suppress
    // weather for a full TTL. Fail instead, so the caller doesn't cache it.
    let status = response.status_code();
    if status >= 400 {
        return Err(format!("{url} returned HTTP {status}"));
    }
    response.json().await.map_err(|e| e.to_string())
}
