use crate::models::{Alert, Precipitation, Weather};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// One hourly NWS forecast period, already parsed down to what we use.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawPeriod {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub temp_f: f64,
    pub humidity_pct: Option<f64>,
    pub wind_mph: Option<f64>,
    pub precip_prob_pct: u8,
    pub short_forecast: String,
}

/// Everything needed to build a `Weather` response: hourly periods already
/// filtered to the hike window, plus any active NWS watches/warnings for the point.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RawForecast {
    pub periods: Vec<RawPeriod>,
    pub active_alerts: Vec<String>,
}

/// Abstraction over the weather backend so handlers can be unit-tested without
/// network. Real impl (`NwsWeatherSource`) calls api.weather.gov; tests use fixtures.
pub trait WeatherSource {
    async fn forecast(
        &self,
        lat: f64,
        lon: f64,
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
    ) -> Result<RawForecast, String>;
}

/// Keeps only periods overlapping `[start, end]`; shared by the real NWS
/// adapter and available here so it stays next to the type it filters.
pub(crate) fn filter_to_window(
    raw: RawForecast,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> RawForecast {
    RawForecast {
        periods: raw
            .periods
            .into_iter()
            .filter(|p| p.start < end && p.end > start)
            .collect(),
        active_alerts: raw.active_alerts,
    }
}

/// Pure parsing of NWS `/points/{lat},{lon}` response -> the hourly forecast URL.
pub(crate) fn forecast_hourly_url(points: &serde_json::Value) -> Result<String, String> {
    points["properties"]["forecastHourly"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "NWS points response missing forecastHourly".to_string())
}

/// Pure parsing of an NWS hourly-forecast response into our internal shape.
/// Periods with unparseable/missing required fields are dropped rather than
/// failing the whole forecast.
pub(crate) fn parse_periods(forecast: &serde_json::Value) -> Vec<RawPeriod> {
    forecast["properties"]["periods"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            Some(RawPeriod {
                start: DateTime::parse_from_rfc3339(p["startTime"].as_str()?)
                    .ok()?
                    .with_timezone(&Utc),
                end: DateTime::parse_from_rfc3339(p["endTime"].as_str()?)
                    .ok()?
                    .with_timezone(&Utc),
                temp_f: p["temperature"].as_f64()?,
                humidity_pct: p["relativeHumidity"]["value"].as_f64(),
                wind_mph: p["windSpeed"].as_str().and_then(parse_wind_mph),
                precip_prob_pct: p["probabilityOfPrecipitation"]["value"]
                    .as_u64()
                    .unwrap_or(0) as u8,
                short_forecast: p["shortForecast"].as_str().unwrap_or_default().to_string(),
            })
        })
        .collect()
}

/// Pure parsing of an NWS `/alerts/active` response into event names.
pub(crate) fn parse_active_alerts(alerts: &serde_json::Value) -> Vec<String> {
    alerts["features"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f["properties"]["event"].as_str().map(str::to_string))
        .collect()
}

/// NWS windSpeed is a string like "10 mph" or "10 to 20 mph"; take the first number.
fn parse_wind_mph(raw: &str) -> Option<f64> {
    raw.split_whitespace().next()?.parse().ok()
}

/// precip probability above which we call it "predicted" for the alert rule.
/// ponytail: any nonzero threshold is a judgment call; tune here if it's noisy.
const PRECIP_ALERT_THRESHOLD_PCT: u8 = 1;
const HEAT_INDEX_ALERT_F: f64 = 85.0;
const WIND_CHILL_ALERT_F: f64 = 32.0;

/// Builds the API's `Weather` block from a raw forecast already filtered to the
/// hike window. Pure function — this is what unit tests exercise.
pub fn build_weather(raw: &RawForecast) -> Option<Weather> {
    if raw.periods.is_empty() {
        return None;
    }

    let max_precip_prob = raw
        .periods
        .iter()
        .map(|p| p.precip_prob_pct)
        .max()
        .unwrap_or(0);
    let representative = &raw.periods[0];

    let heat_index_f = raw
        .periods
        .iter()
        .filter(|p| p.temp_f >= 80.0)
        .filter_map(|p| p.humidity_pct.map(|h| heat_index(p.temp_f, h)))
        .fold(None, |acc: Option<f64>, hi| {
            Some(acc.map_or(hi, |a| a.max(hi)))
        });

    let wind_chill_f = raw
        .periods
        .iter()
        .filter(|p| p.temp_f <= 50.0)
        .filter_map(|p| p.wind_mph.map(|w| (p.temp_f, w)))
        .filter(|(_, wind)| *wind > 3.0)
        .map(|(temp, wind)| wind_chill(temp, wind))
        .fold(None, |acc: Option<f64>, wc| {
            Some(acc.map_or(wc, |a| a.min(wc)))
        });

    let mut alerts = Vec::new();
    if max_precip_prob >= PRECIP_ALERT_THRESHOLD_PCT {
        alerts.push(Alert {
            kind: "precip".to_string(),
            message: format!(
                "Precipitation predicted during the hike window ({max_precip_prob}% chance)"
            ),
        });
    }
    for nws_alert in &raw.active_alerts {
        alerts.push(Alert {
            kind: "nws_alert".to_string(),
            message: nws_alert.clone(),
        });
    }
    if let Some(hi) = heat_index_f.filter(|hi| *hi > HEAT_INDEX_ALERT_F) {
        alerts.push(Alert {
            kind: "heat_index".to_string(),
            message: format!("Heat index of {hi:.0}\u{b0}F exceeds {HEAT_INDEX_ALERT_F:.0}\u{b0}F"),
        });
    }
    if let Some(wc) = wind_chill_f.filter(|wc| *wc < WIND_CHILL_ALERT_F) {
        alerts.push(Alert {
            kind: "wind_chill".to_string(),
            message: format!(
                "Wind chill of {wc:.0}\u{b0}F is below {WIND_CHILL_ALERT_F:.0}\u{b0}F"
            ),
        });
    }

    Some(Weather {
        temperature_f: representative.temp_f,
        conditions: representative.short_forecast.clone(),
        precipitation: Precipitation {
            probability_pct: max_precip_prob,
            // ponytail: NWS hourly forecast doesn't expose quantitative precip
            // amount, only probability. Upgrade: pull QPF from the /gridpoint
            // endpoint if an amount estimate becomes worth the extra fetch.
            amount_in: 0.0,
        },
        heat_index_f,
        wind_chill_f,
        alerts,
    })
}

/// NWS Rothfusz regression, °F + relative humidity % -> heat index °F.
pub fn heat_index(temp_f: f64, humidity_pct: f64) -> f64 {
    let t = temp_f;
    let r = humidity_pct;
    let simple = 0.5 * (t + 61.0 + (t - 68.0) * 1.2 + r * 0.094);
    if simple < 80.0 {
        return simple;
    }
    -42.379 + 2.04901523 * t + 10.14333127 * r
        - 0.22475541 * t * r
        - 0.00683783 * t * t
        - 0.05481717 * r * r
        + 0.00122874 * t * t * r
        + 0.00085282 * t * r * r
        - 0.00000199 * t * t * r * r
}

/// NWS wind chill formula, °F + wind speed mph -> wind chill °F.
pub fn wind_chill(temp_f: f64, wind_mph: f64) -> f64 {
    let t = temp_f;
    let v = wind_mph.powf(0.16);
    35.74 + 0.6215 * t - 35.75 * v + 0.4275 * t * v
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn period(temp_f: f64, humidity_pct: f64, wind_mph: f64, precip_prob_pct: u8) -> RawPeriod {
        RawPeriod {
            start: Utc.with_ymd_and_hms(2026, 7, 18, 8, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 7, 18, 9, 0, 0).unwrap(),
            temp_f,
            humidity_pct: Some(humidity_pct),
            wind_mph: Some(wind_mph),
            precip_prob_pct,
            short_forecast: "Partly Cloudy".to_string(),
        }
    }

    #[test]
    fn no_periods_means_no_weather() {
        assert!(build_weather(&RawForecast::default()).is_none());
    }

    #[test]
    fn forecast_hourly_url_extracts_from_points_response() {
        let points = serde_json::json!({
            "properties": { "forecastHourly": "https://api.weather.gov/gridpoints/LWX/1,1/forecast/hourly" }
        });
        assert_eq!(
            forecast_hourly_url(&points).unwrap(),
            "https://api.weather.gov/gridpoints/LWX/1,1/forecast/hourly"
        );
    }

    #[test]
    fn forecast_hourly_url_errors_when_missing() {
        assert!(forecast_hourly_url(&serde_json::json!({})).is_err());
    }

    #[test]
    fn parse_periods_reads_real_nws_shape() {
        let forecast = serde_json::json!({
            "properties": {
                "periods": [{
                    "startTime": "2026-07-18T08:00:00-04:00",
                    "endTime": "2026-07-18T09:00:00-04:00",
                    "temperature": 78,
                    "relativeHumidity": { "value": 55.0 },
                    "windSpeed": "10 mph",
                    "probabilityOfPrecipitation": { "value": 20 },
                    "shortForecast": "Partly Cloudy"
                }]
            }
        });
        let periods = parse_periods(&forecast);
        assert_eq!(periods.len(), 1);
        assert_eq!(periods[0].temp_f, 78.0);
        assert_eq!(periods[0].humidity_pct, Some(55.0));
        assert_eq!(periods[0].wind_mph, Some(10.0));
        assert_eq!(periods[0].precip_prob_pct, 20);
        assert_eq!(periods[0].short_forecast, "Partly Cloudy");
    }

    #[test]
    fn parse_periods_drops_entries_missing_required_fields() {
        let forecast = serde_json::json!({
            "properties": { "periods": [{ "temperature": 78 }] }
        });
        assert!(parse_periods(&forecast).is_empty());
    }

    #[test]
    fn parse_periods_handles_missing_array() {
        assert!(parse_periods(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn parse_active_alerts_reads_event_names() {
        let alerts = serde_json::json!({
            "features": [
                { "properties": { "event": "Flash Flood Watch" } },
                { "properties": { "event": "Heat Advisory" } }
            ]
        });
        assert_eq!(
            parse_active_alerts(&alerts),
            vec!["Flash Flood Watch".to_string(), "Heat Advisory".to_string()]
        );
    }

    #[test]
    fn parse_active_alerts_handles_missing_array() {
        assert!(parse_active_alerts(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn parse_wind_mph_handles_range_format() {
        assert_eq!(parse_wind_mph("10 to 20 mph"), Some(10.0));
        assert_eq!(parse_wind_mph("10 mph"), Some(10.0));
        assert_eq!(parse_wind_mph(""), None);
    }

    #[test]
    fn filter_to_window_keeps_only_overlapping_periods() {
        let raw = RawForecast {
            periods: vec![
                period(70.0, 40.0, 5.0, 0), // 08:00-09:00
            ],
            active_alerts: vec!["Heat Advisory".to_string()],
        };
        let start = Utc.with_ymd_and_hms(2026, 7, 18, 8, 30, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2026, 7, 18, 12, 0, 0).unwrap();
        let filtered = filter_to_window(raw, start, end);
        assert_eq!(filtered.periods.len(), 1);
        assert_eq!(filtered.active_alerts, vec!["Heat Advisory".to_string()]);

        let raw_outside = RawForecast {
            periods: vec![period(70.0, 40.0, 5.0, 0)],
            active_alerts: vec![],
        };
        let far_start = Utc.with_ymd_and_hms(2026, 7, 19, 8, 0, 0).unwrap();
        let far_end = Utc.with_ymd_and_hms(2026, 7, 19, 12, 0, 0).unwrap();
        assert!(
            filter_to_window(raw_outside, far_start, far_end)
                .periods
                .is_empty()
        );
    }

    #[test]
    fn mild_conditions_produce_no_alerts() {
        let raw = RawForecast {
            periods: vec![period(70.0, 40.0, 5.0, 0)],
            active_alerts: vec![],
        };
        let w = build_weather(&raw).unwrap();
        assert!(w.alerts.is_empty());
        assert!(w.heat_index_f.is_none());
        assert!(w.wind_chill_f.is_none());
    }

    #[test]
    fn hot_humid_triggers_heat_index_alert() {
        let raw = RawForecast {
            periods: vec![period(95.0, 70.0, 5.0, 0)],
            active_alerts: vec![],
        };
        let w = build_weather(&raw).unwrap();
        assert!(w.heat_index_f.unwrap() > HEAT_INDEX_ALERT_F);
        assert!(w.alerts.iter().any(|a| a.kind == "heat_index"));
    }

    #[test]
    fn cold_windy_triggers_wind_chill_alert() {
        let raw = RawForecast {
            periods: vec![period(20.0, 40.0, 15.0, 0)],
            active_alerts: vec![],
        };
        let w = build_weather(&raw).unwrap();
        assert!(w.wind_chill_f.unwrap() < WIND_CHILL_ALERT_F);
        assert!(w.alerts.iter().any(|a| a.kind == "wind_chill"));
    }

    #[test]
    fn any_precip_probability_triggers_precip_alert() {
        let raw = RawForecast {
            periods: vec![period(70.0, 40.0, 5.0, 20)],
            active_alerts: vec![],
        };
        let w = build_weather(&raw).unwrap();
        assert!(w.alerts.iter().any(|a| a.kind == "precip"));
    }

    #[test]
    fn active_nws_alerts_pass_through() {
        let raw = RawForecast {
            periods: vec![period(70.0, 40.0, 5.0, 0)],
            active_alerts: vec!["Flash Flood Watch".to_string()],
        };
        let w = build_weather(&raw).unwrap();
        assert!(
            w.alerts
                .iter()
                .any(|a| a.kind == "nws_alert" && a.message == "Flash Flood Watch")
        );
    }

    #[test]
    fn heat_index_matches_known_value() {
        // NWS Rothfusz regression at 95F/70% RH evaluates to ~122.6F.
        let hi = heat_index(95.0, 70.0);
        assert!((120.0..=125.0).contains(&hi), "got {hi}");
    }

    #[test]
    fn wind_chill_matches_known_value() {
        // NWS reference: 20F at 15mph wind chill ~= 6F.
        let wc = wind_chill(20.0, 15.0);
        assert!((4.0..=8.0).contains(&wc), "got {wc}");
    }
}
