use crate::models::{Alert, Precipitation, PrecipitationV2, Weather, WeatherV2};
use chrono::{DateTime, FixedOffset, Utc};
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

/// An active NWS watch/warning/advisory with its validity window, so builders
/// can decide whether it's relevant to a given hike time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawAlert {
    pub event: String,
    pub onset: Option<DateTime<Utc>>,
    pub ends: Option<DateTime<Utc>>,
}

/// Everything needed to build a weather response: the *full* hourly forecast
/// (unfiltered — builders narrow to the window they need) plus the point's
/// active NWS alerts with their validity windows.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RawForecast {
    pub periods: Vec<RawPeriod>,
    pub alerts: Vec<RawAlert>,
}

/// Abstraction over the weather backend so handlers can be unit-tested without
/// network. Real impl (`NwsWeatherSource`) calls api.weather.gov; tests use fixtures.
pub trait WeatherSource {
    /// Fetches the point's *full* forecast (all hourly periods + active alerts);
    /// builders narrow to the hike window themselves.
    async fn forecast(&self, lat: f64, lon: f64) -> Result<RawForecast, String>;
}

/// Periods overlapping `[start, end]`, borrowed from the full forecast.
fn periods_in_window(
    periods: &[RawPeriod],
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Vec<&RawPeriod> {
    periods
        .iter()
        .filter(|p| p.start < end && p.end > start)
        .collect()
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

/// Pure parsing of an NWS `/alerts/active` response into events with their
/// validity windows. `onset`/`ends` fall back to `effective`/`expires` (NWS
/// populates one or the other); a missing bound is left `None` and treated as
/// open-ended by consumers.
pub(crate) fn parse_active_alerts(alerts: &serde_json::Value) -> Vec<RawAlert> {
    alerts["features"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| {
            let props = &f["properties"];
            let event = props["event"].as_str()?.to_string();
            Some(RawAlert {
                event,
                onset: parse_alert_time(props, "onset", "effective"),
                ends: parse_alert_time(props, "ends", "expires"),
            })
        })
        .collect()
}

/// Reads an RFC 3339 alert timestamp, preferring `primary` then falling back to
/// `fallback`; unparseable/missing → `None`.
fn parse_alert_time(
    props: &serde_json::Value,
    primary: &str,
    fallback: &str,
) -> Option<DateTime<Utc>> {
    props[primary]
        .as_str()
        .or_else(|| props[fallback].as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

/// NWS windSpeed is a string like "10 mph" or "10 to 20 mph"; take the first number.
fn parse_wind_mph(raw: &str) -> Option<f64> {
    raw.split_whitespace().next()?.parse().ok()
}

/// precip probability above which we call it "predicted" for the v1 alert rule.
/// ponytail: any nonzero threshold is a judgment call; tune here if it's noisy.
const PRECIP_ALERT_THRESHOLD_PCT: u8 = 1;
/// v2 precip-*timing* threshold: only hours this likely count as "rain expected".
const PRECIP_LIKELY_THRESHOLD_PCT: u8 = 50;
const HEAT_INDEX_ALERT_F: f64 = 85.0;
const WIND_CHILL_ALERT_F: f64 = 32.0;

fn max_precip_prob(periods: &[&RawPeriod]) -> u8 {
    periods.iter().map(|p| p.precip_prob_pct).max().unwrap_or(0)
}

fn max_heat_index(periods: &[&RawPeriod]) -> Option<f64> {
    periods
        .iter()
        .filter(|p| p.temp_f >= 80.0)
        .filter_map(|p| p.humidity_pct.map(|h| heat_index(p.temp_f, h)))
        .fold(None, |acc, hi| Some(acc.map_or(hi, |a: f64| a.max(hi))))
}

fn min_wind_chill(periods: &[&RawPeriod]) -> Option<f64> {
    periods
        .iter()
        .filter(|p| p.temp_f <= 50.0)
        .filter_map(|p| p.wind_mph.map(|w| (p.temp_f, w)))
        .filter(|(_, wind)| *wind > 3.0)
        .map(|(temp, wind)| wind_chill(temp, wind))
        .fold(None, |acc, wc| Some(acc.map_or(wc, |a: f64| a.min(wc))))
}

fn precip_alert(max_prob: u8) -> Option<Alert> {
    (max_prob >= PRECIP_ALERT_THRESHOLD_PCT).then(|| Alert {
        kind: "precip".to_string(),
        message: format!("Precipitation predicted during the hike window ({max_prob}% chance)"),
    })
}

fn heat_alert(heat_index_f: Option<f64>) -> Option<Alert> {
    heat_index_f
        .filter(|hi| *hi > HEAT_INDEX_ALERT_F)
        .map(|hi| Alert {
            kind: "heat_index".to_string(),
            message: format!("Heat index of {hi:.0}\u{b0}F exceeds {HEAT_INDEX_ALERT_F:.0}\u{b0}F"),
        })
}

fn wind_chill_alert(wind_chill_f: Option<f64>) -> Option<Alert> {
    wind_chill_f
        .filter(|wc| *wc < WIND_CHILL_ALERT_F)
        .map(|wc| Alert {
            kind: "wind_chill".to_string(),
            message: format!(
                "Wind chill of {wc:.0}\u{b0}F is below {WIND_CHILL_ALERT_F:.0}\u{b0}F"
            ),
        })
}

/// Builds the **v1** `Weather` block for the hike window. Behavior preserved from
/// the original: single temp (first period), passes through *all* active NWS
/// alerts. Now filters the full forecast to the window itself.
pub fn build_weather(
    raw: &RawForecast,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Option<Weather> {
    let periods = periods_in_window(&raw.periods, start, end);
    let representative = *periods.first()?;

    let max_prob = max_precip_prob(&periods);
    let heat_index_f = max_heat_index(&periods);
    let wind_chill_f = min_wind_chill(&periods);

    let mut alerts = Vec::new();
    alerts.extend(precip_alert(max_prob));
    // v1 passes through every active NWS alert unchanged (no time filtering).
    alerts.extend(raw.alerts.iter().map(|a| Alert {
        kind: "nws_alert".to_string(),
        message: a.event.clone(),
    }));
    alerts.extend(heat_alert(heat_index_f));
    alerts.extend(wind_chill_alert(wind_chill_f));

    Some(Weather {
        temperature_f: representative.temp_f,
        conditions: representative.short_forecast.clone(),
        precipitation: Precipitation {
            probability_pct: max_prob,
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

/// Builds the **v2** `WeatherV2` block: start/end temps, precip timing across the
/// hike's local calendar day, and NWS alerts filtered to those overlapping the
/// hike window.
pub fn build_weather_v2(
    raw: &RawForecast,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    offset: FixedOffset,
) -> Option<WeatherV2> {
    let window = periods_in_window(&raw.periods, start, end);
    let first = *window.first()?;
    let last = *window.last()?;

    let max_prob = max_precip_prob(&window);
    let heat_index_f = max_heat_index(&window);
    let wind_chill_f = min_wind_chill(&window);

    let mut alerts = Vec::new();
    alerts.extend(precip_alert(max_prob));
    // v2 keeps only alerts whose [onset, ends] overlaps the hike window; a
    // missing bound is treated as open-ended (always overlapping on that side).
    alerts.extend(
        raw.alerts
            .iter()
            .filter(|a| a.onset.is_none_or(|o| o < end) && a.ends.is_none_or(|e| e > start))
            .map(|a| Alert {
                kind: "nws_alert".to_string(),
                message: a.event.clone(),
            }),
    );
    alerts.extend(heat_alert(heat_index_f));
    alerts.extend(wind_chill_alert(wind_chill_f));

    Some(WeatherV2 {
        start_temp_f: first.temp_f,
        end_temp_f: last.temp_f,
        conditions: first.short_forecast.clone(),
        precipitation: precip_timing(&raw.periods, start, offset, max_prob),
        heat_index_f,
        wind_chill_f,
        alerts,
    })
}

/// Precip timing over the hike's *local calendar day* (the day of `start` in its
/// own offset): earliest start / latest end among hours at/above the "likely"
/// threshold. Timestamps are emitted in the hike's local offset so they read
/// naturally, and may fall before/after the hike window.
fn precip_timing(
    periods: &[RawPeriod],
    start: DateTime<Utc>,
    offset: FixedOffset,
    window_max_prob: u8,
) -> PrecipitationV2 {
    let day = start.with_timezone(&offset).date_naive();

    let likely: Vec<&RawPeriod> = periods
        .iter()
        .filter(|p| p.precip_prob_pct >= PRECIP_LIKELY_THRESHOLD_PCT)
        .filter(|p| p.start.with_timezone(&offset).date_naive() == day)
        .collect();

    let starts_at = likely
        .iter()
        .map(|p| p.start)
        .min()
        .map(|t| t.with_timezone(&offset).to_rfc3339());
    let ends_at = likely
        .iter()
        .map(|p| p.end)
        .max()
        .map(|t| t.with_timezone(&offset).to_rfc3339());

    PrecipitationV2 {
        probability_pct: window_max_prob,
        expected: !likely.is_empty(),
        starts_at,
        ends_at,
    }
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

    /// UTC instant on the hike's test day.
    fn at(hour: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 18, hour, min, 0).unwrap()
    }

    /// One-hour period starting at `hour:00` UTC.
    fn hour(hour: u32, temp_f: f64, humidity_pct: f64, wind_mph: f64, precip: u8) -> RawPeriod {
        RawPeriod {
            start: at(hour, 0),
            end: at(hour + 1, 0),
            temp_f,
            humidity_pct: Some(humidity_pct),
            wind_mph: Some(wind_mph),
            precip_prob_pct: precip,
            short_forecast: format!("Hour {hour}"),
        }
    }

    fn alert(event: &str, onset: Option<DateTime<Utc>>, ends: Option<DateTime<Utc>>) -> RawAlert {
        RawAlert {
            event: event.to_string(),
            onset,
            ends,
        }
    }

    const UTC: FixedOffset = match FixedOffset::east_opt(0) {
        Some(o) => o,
        None => unreachable!(),
    };

    // one 08:00-09:00 period, window covering it
    fn one_period(temp_f: f64, humidity_pct: f64, wind_mph: f64, precip: u8) -> RawForecast {
        RawForecast {
            periods: vec![hour(8, temp_f, humidity_pct, wind_mph, precip)],
            alerts: vec![],
        }
    }

    #[test]
    fn no_periods_means_no_weather() {
        assert!(build_weather(&RawForecast::default(), at(8, 0), at(9, 0)).is_none());
        assert!(build_weather_v2(&RawForecast::default(), at(8, 0), at(9, 0), UTC).is_none());
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
    fn parse_active_alerts_reads_events_and_times() {
        let alerts = serde_json::json!({
            "features": [
                { "properties": {
                    "event": "Flash Flood Watch",
                    "onset": "2026-07-18T08:00:00-04:00",
                    "ends": "2026-07-18T20:00:00-04:00"
                } },
                // falls back to effective/expires when onset/ends absent
                { "properties": {
                    "event": "Heat Advisory",
                    "effective": "2026-07-18T10:00:00-04:00",
                    "expires": "2026-07-18T18:00:00-04:00"
                } }
            ]
        });
        let parsed = parse_active_alerts(&alerts);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].event, "Flash Flood Watch");
        assert_eq!(
            parsed[0].onset,
            Some("2026-07-18T12:00:00Z".parse().unwrap())
        );
        assert_eq!(parsed[1].event, "Heat Advisory");
        assert_eq!(
            parsed[1].onset,
            Some("2026-07-18T14:00:00Z".parse().unwrap())
        );
        assert_eq!(
            parsed[1].ends,
            Some("2026-07-18T22:00:00Z".parse().unwrap())
        );
    }

    #[test]
    fn parse_active_alerts_handles_missing_array_and_times() {
        assert!(parse_active_alerts(&serde_json::json!({})).is_empty());
        let no_times = serde_json::json!({ "features": [{ "properties": { "event": "X" } }] });
        let parsed = parse_active_alerts(&no_times);
        assert_eq!(parsed[0].onset, None);
        assert_eq!(parsed[0].ends, None);
    }

    #[test]
    fn parse_wind_mph_handles_range_format() {
        assert_eq!(parse_wind_mph("10 to 20 mph"), Some(10.0));
        assert_eq!(parse_wind_mph("10 mph"), Some(10.0));
        assert_eq!(parse_wind_mph(""), None);
    }

    #[test]
    fn build_weather_ignores_periods_outside_the_window() {
        // 08:00 period is in-window; a hot 20:00 period must not leak into metrics.
        let raw = RawForecast {
            periods: vec![hour(8, 70.0, 40.0, 5.0, 0), hour(20, 99.0, 90.0, 5.0, 80)],
            alerts: vec![],
        };
        let w = build_weather(&raw, at(8, 0), at(9, 0)).unwrap();
        assert_eq!(w.temperature_f, 70.0);
        assert_eq!(w.precipitation.probability_pct, 0);
        assert!(w.alerts.is_empty());
    }

    #[test]
    fn mild_conditions_produce_no_alerts() {
        let w = build_weather(&one_period(70.0, 40.0, 5.0, 0), at(8, 0), at(9, 0)).unwrap();
        assert!(w.alerts.is_empty());
        assert!(w.heat_index_f.is_none());
        assert!(w.wind_chill_f.is_none());
    }

    #[test]
    fn hot_humid_triggers_heat_index_alert() {
        let w = build_weather(&one_period(95.0, 70.0, 5.0, 0), at(8, 0), at(9, 0)).unwrap();
        assert!(w.heat_index_f.unwrap() > HEAT_INDEX_ALERT_F);
        assert!(w.alerts.iter().any(|a| a.kind == "heat_index"));
    }

    #[test]
    fn cold_windy_triggers_wind_chill_alert() {
        let w = build_weather(&one_period(20.0, 40.0, 15.0, 0), at(8, 0), at(9, 0)).unwrap();
        assert!(w.wind_chill_f.unwrap() < WIND_CHILL_ALERT_F);
        assert!(w.alerts.iter().any(|a| a.kind == "wind_chill"));
    }

    #[test]
    fn any_precip_probability_triggers_precip_alert() {
        let w = build_weather(&one_period(70.0, 40.0, 5.0, 20), at(8, 0), at(9, 0)).unwrap();
        assert!(w.alerts.iter().any(|a| a.kind == "precip"));
    }

    #[test]
    fn v1_passes_through_all_active_nws_alerts_regardless_of_time() {
        let raw = RawForecast {
            periods: vec![hour(8, 70.0, 40.0, 5.0, 0)],
            // ends long before the hike — v1 still passes it through (frozen behavior)
            alerts: vec![alert("Flash Flood Watch", Some(at(0, 0)), Some(at(2, 0)))],
        };
        let w = build_weather(&raw, at(8, 0), at(9, 0)).unwrap();
        assert!(
            w.alerts
                .iter()
                .any(|a| a.kind == "nws_alert" && a.message == "Flash Flood Watch")
        );
    }

    #[test]
    fn v2_reports_start_and_end_temps() {
        let raw = RawForecast {
            periods: vec![
                hour(8, 70.0, 40.0, 5.0, 0),
                hour(9, 75.0, 40.0, 5.0, 0),
                hour(10, 80.0, 40.0, 5.0, 0),
            ],
            alerts: vec![],
        };
        let w = build_weather_v2(&raw, at(8, 0), at(11, 0), UTC).unwrap();
        assert_eq!(w.start_temp_f, 70.0);
        assert_eq!(w.end_temp_f, 80.0);
        assert_eq!(w.conditions, "Hour 8");
    }

    #[test]
    fn v2_precip_timing_detects_rain_before_the_hike() {
        // Rain 06:00-08:00 (>=50%), hike 08:00-11:00 dry. Timing spans the pre-hike
        // rain; probabilityPct (during the window) stays 0.
        let raw = RawForecast {
            periods: vec![
                hour(6, 65.0, 80.0, 5.0, 70),
                hour(7, 66.0, 80.0, 5.0, 60),
                hour(8, 70.0, 40.0, 5.0, 0),
                hour(9, 72.0, 40.0, 5.0, 0),
                hour(10, 74.0, 40.0, 5.0, 0),
            ],
            alerts: vec![],
        };
        let w = build_weather_v2(&raw, at(8, 0), at(11, 0), UTC).unwrap();
        assert!(w.precipitation.expected);
        assert_eq!(w.precipitation.probability_pct, 0);
        assert_eq!(
            w.precipitation.starts_at.as_deref(),
            Some("2026-07-18T06:00:00+00:00")
        );
        assert_eq!(
            w.precipitation.ends_at.as_deref(),
            Some("2026-07-18T08:00:00+00:00")
        );
    }

    #[test]
    fn v2_precip_timing_empty_when_no_likely_hours() {
        let raw = RawForecast {
            periods: vec![hour(8, 70.0, 40.0, 5.0, 30)], // 30% < likely threshold
            alerts: vec![],
        };
        let w = build_weather_v2(&raw, at(8, 0), at(9, 0), UTC).unwrap();
        assert!(!w.precipitation.expected);
        assert!(w.precipitation.starts_at.is_none());
        assert!(w.precipitation.ends_at.is_none());
    }

    #[test]
    fn v2_precip_timing_respects_local_calendar_day() {
        // Offset -04:00: hike starts 2026-07-18T12:00Z == 08:00 local (day 07-18).
        let offset = FixedOffset::west_opt(4 * 3600).unwrap();
        let raw = RawForecast {
            periods: vec![
                // 2026-07-18T02:00Z == 2026-07-17T22:00 local — previous day, excluded
                RawPeriod {
                    start: "2026-07-18T02:00:00Z".parse().unwrap(),
                    end: "2026-07-18T03:00:00Z".parse().unwrap(),
                    temp_f: 60.0,
                    humidity_pct: Some(80.0),
                    wind_mph: Some(5.0),
                    precip_prob_pct: 90,
                    short_forecast: "Rain".to_string(),
                },
                // 2026-07-18T13:00Z == 09:00 local — same day, included
                RawPeriod {
                    start: "2026-07-18T13:00:00Z".parse().unwrap(),
                    end: "2026-07-18T14:00:00Z".parse().unwrap(),
                    temp_f: 70.0,
                    humidity_pct: Some(80.0),
                    wind_mph: Some(5.0),
                    precip_prob_pct: 90,
                    short_forecast: "Rain".to_string(),
                },
            ],
            alerts: vec![],
        };
        let start = "2026-07-18T12:00:00Z".parse().unwrap();
        let end = "2026-07-18T16:00:00Z".parse().unwrap();
        let w = build_weather_v2(&raw, start, end, offset).unwrap();
        assert_eq!(
            w.precipitation.starts_at.as_deref(),
            Some("2026-07-18T09:00:00-04:00")
        );
        assert_eq!(
            w.precipitation.ends_at.as_deref(),
            Some("2026-07-18T10:00:00-04:00")
        );
    }

    #[test]
    fn v2_filters_alerts_to_the_hike_window() {
        let raw = RawForecast {
            periods: vec![hour(8, 70.0, 40.0, 5.0, 0)],
            alerts: vec![
                alert("Overlaps", Some(at(8, 30)), Some(at(10, 0))),
                alert("Before hike", Some(at(0, 0)), Some(at(2, 0))),
                alert("Open ended", None, None),
            ],
        };
        let w = build_weather_v2(&raw, at(8, 0), at(9, 0), UTC).unwrap();
        let events: Vec<&str> = w
            .alerts
            .iter()
            .filter(|a| a.kind == "nws_alert")
            .map(|a| a.message.as_str())
            .collect();
        assert!(events.contains(&"Overlaps"));
        assert!(events.contains(&"Open ended"));
        assert!(!events.contains(&"Before hike"));
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
