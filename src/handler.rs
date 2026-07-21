use crate::models::{HikeResponse, HikeResponseV2, MapRef, MeetingPoint};
use crate::r2::HikeStore;
use crate::version::ApiVersion;
use crate::weather::{WeatherSource, build_weather, build_weather_v2};
use chrono::DateTime;

/// A hike response in the shape for the requested API version. `lib.rs` matches
/// on this and serializes the appropriate variant.
pub enum VersionedHike {
    V1(HikeResponse),
    V2(HikeResponseV2),
}

/// Pure orchestration: fetch hike metadata + the full forecast, assemble the
/// version-appropriate response. Generic over both traits so tests inject
/// fixtures with zero network.
pub async fn build_hike_response<S: HikeStore, W: WeatherSource>(
    store: &S,
    weather_source: &W,
    id: &str,
    version: ApiVersion,
) -> Result<Option<VersionedHike>, String> {
    let Some(record) = store.get_hike(id).await? else {
        return Ok(None);
    };

    let (map_url, expires_at) = store.presign_map_url(&record.map_key).await?;

    // Parse preserving the offset (needed for v2's local-calendar-day precip
    // timing); the instants drive window filtering.
    let start_local = DateTime::parse_from_rfc3339(&record.start).map_err(|e| e.to_string())?;
    let end_local = DateTime::parse_from_rfc3339(&record.end).map_err(|e| e.to_string())?;
    let offset = start_local.timezone();
    let start = start_local.to_utc();
    let end = end_local.to_utc();

    let forecast = weather_source
        .forecast(record.meeting.lat, record.meeting.lon)
        .await;
    let raw = forecast.as_ref().ok();

    let meeting_point = MeetingPoint::new(record.meeting.lat, record.meeting.lon);
    let map = MapRef {
        url: map_url,
        expires_at: expires_at.to_rfc3339(),
    };

    let response = match version {
        ApiVersion::V1 => {
            let weather = raw.and_then(|raw| build_weather(raw, start, end));
            VersionedHike::V1(HikeResponse {
                id: record.id,
                start: record.start,
                end: record.end,
                meeting_point,
                trails: record.trails,
                map,
                weather_available: weather.is_some(),
                weather,
            })
        }
        ApiVersion::V2 => {
            let weather = raw.and_then(|raw| build_weather_v2(raw, start, end, offset));
            VersionedHike::V2(HikeResponseV2 {
                id: record.id,
                start: record.start,
                end: record.end,
                meeting_point,
                trails: record.trails,
                map,
                weather_available: weather.is_some(),
                weather,
            })
        }
    };

    Ok(Some(response))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{HikeRecord, MeetingCoords};
    use crate::weather::RawForecast;

    struct FixtureStore {
        record: Option<HikeRecord>,
    }

    impl HikeStore for FixtureStore {
        async fn get_hike(&self, _id: &str) -> Result<Option<HikeRecord>, String> {
            Ok(self.record.clone())
        }

        async fn presign_map_url(
            &self,
            _map_key: &str,
        ) -> Result<(String, chrono::DateTime<chrono::Utc>), String> {
            Ok((
                "https://example.com/map.png".to_string(),
                chrono::Utc::now(),
            ))
        }
    }

    struct FixtureWeather {
        result: Result<RawForecast, String>,
    }

    impl WeatherSource for FixtureWeather {
        async fn forecast(&self, _lat: f64, _lon: f64) -> Result<RawForecast, String> {
            self.result.clone()
        }
    }

    /// Unwrap a `VersionedHike` known to be V1 for assertions.
    fn v1(h: VersionedHike) -> HikeResponse {
        match h {
            VersionedHike::V1(r) => r,
            VersionedHike::V2(_) => panic!("expected v1 response"),
        }
    }

    fn sample_record() -> HikeRecord {
        HikeRecord {
            id: "2026-07-18-blue-ridge".to_string(),
            start: "2026-07-18T08:00:00-04:00".to_string(),
            end: "2026-07-18T12:00:00-04:00".to_string(),
            meeting: MeetingCoords {
                lat: 37.6,
                lon: -79.2,
            },
            trails: vec!["Blue Ridge Loop".to_string()],
            map_key: "hikes/2026-07-18-blue-ridge/map.png".to_string(),
        }
    }

    /// A forecast whose single period covers the sample hike window (08:00-12:00
    /// local == 12:00-16:00Z).
    fn sample_forecast() -> RawForecast {
        RawForecast {
            periods: vec![crate::weather::RawPeriod {
                start: "2026-07-18T12:00:00Z".parse().unwrap(),
                end: "2026-07-18T16:00:00Z".parse().unwrap(),
                temp_f: 78.0,
                humidity_pct: Some(50.0),
                wind_mph: Some(5.0),
                precip_prob_pct: 10,
                short_forecast: "Partly Cloudy".to_string(),
            }],
            alerts: vec![],
        }
    }

    #[tokio::test]
    async fn missing_hike_returns_none() {
        let store = FixtureStore { record: None };
        let weather = FixtureWeather {
            result: Ok(RawForecast::default()),
        };
        let result = build_hike_response(&store, &weather, "nope", ApiVersion::V1)
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn found_hike_with_weather_failure_is_best_effort() {
        let store = FixtureStore {
            record: Some(sample_record()),
        };
        let weather = FixtureWeather {
            result: Err("nws down".to_string()),
        };
        let response = v1(build_hike_response(&store, &weather, "x", ApiVersion::V1)
            .await
            .unwrap()
            .unwrap());
        assert!(!response.weather_available);
        assert!(response.weather.is_none());
        assert_eq!(response.id, "2026-07-18-blue-ridge");
    }

    #[tokio::test]
    async fn v1_populates_weather() {
        let store = FixtureStore {
            record: Some(sample_record()),
        };
        let weather = FixtureWeather {
            result: Ok(sample_forecast()),
        };
        let response = v1(build_hike_response(&store, &weather, "x", ApiVersion::V1)
            .await
            .unwrap()
            .unwrap());
        assert!(response.weather_available);
        assert_eq!(response.weather.unwrap().conditions, "Partly Cloudy");
    }

    #[tokio::test]
    async fn v2_returns_v2_shape() {
        let store = FixtureStore {
            record: Some(sample_record()),
        };
        let weather = FixtureWeather {
            result: Ok(sample_forecast()),
        };
        let response = build_hike_response(&store, &weather, "x", ApiVersion::V2)
            .await
            .unwrap()
            .unwrap();
        match response {
            VersionedHike::V2(r) => {
                let w = r.weather.unwrap();
                assert_eq!(w.start_temp_f, 78.0);
                assert_eq!(w.end_temp_f, 78.0);
            }
            VersionedHike::V1(_) => panic!("expected v2 response"),
        }
    }
}
