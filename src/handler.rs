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
// @spec API-RESP-001, API-RESP-002, API-RESP-003, API-RESP-004, API-RESP-005,
// @spec API-RESP-006, API-RESP-007, API-RESP-008, API-RESP-009, API-RESP-011
pub async fn build_hike_response<S: HikeStore, W: WeatherSource>(
    store: &S,
    weather_source: &W,
    id: &str,
    version: ApiVersion,
) -> Result<Option<VersionedHike>, String> {
    let Some(record) = store.get_hike(id).await? else {
        return Ok(None);
    };

    // ponytail: every served shape has a non-null map, so a missing one is a 502
    // under all of them until API-RESP-010 gives v3 a nullable map.
    let (map_url, expires_at) = store
        .presign_map_url(&record.map_key)
        .await?
        .ok_or_else(|| format!("map object not found: {}", record.map_key))?;

    // Parse preserving the offset (needed for v2's local-calendar-day precip
    // timing); the instants drive window filtering.
    let start_local = DateTime::parse_from_rfc3339(&record.start).map_err(|e| e.to_string())?;
    let end_local = DateTime::parse_from_rfc3339(&record.end).map_err(|e| e.to_string())?;
    let offset = start_local.timezone();
    let start = start_local.to_utc();
    let end = end_local.to_utc();

    let forecast = weather_source
        .forecast(record.meeting.lat, record.meeting.lon, start, end)
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
        // ponytail: v3 renders v2's shape until API-WIRE-004 and API-WIRE-009
        // give it its own weather block and nullable map.
        ApiVersion::V2 | ApiVersion::V3 => {
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
        ) -> Result<Option<(String, chrono::DateTime<chrono::Utc>)>, String> {
            Ok(Some((
                "https://example.com/map.png".to_string(),
                chrono::Utc::now(),
            )))
        }
    }

    /// A store whose reads fail, for the path that must not degrade.
    struct FailingStore;

    impl HikeStore for FailingStore {
        async fn get_hike(&self, _id: &str) -> Result<Option<HikeRecord>, String> {
            Err("r2 unavailable".to_string())
        }

        async fn presign_map_url(
            &self,
            _map_key: &str,
        ) -> Result<Option<(String, chrono::DateTime<chrono::Utc>)>, String> {
            Err("r2 unavailable".to_string())
        }
    }

    /// A store whose record names a map object that was never uploaded.
    struct MaplessStore;

    impl HikeStore for MaplessStore {
        async fn get_hike(&self, _id: &str) -> Result<Option<HikeRecord>, String> {
            Ok(Some(sample_record()))
        }

        async fn presign_map_url(
            &self,
            _map_key: &str,
        ) -> Result<Option<(String, chrono::DateTime<chrono::Utc>)>, String> {
            Ok(None)
        }
    }

    struct FixtureWeather {
        result: Result<RawForecast, String>,
    }

    impl WeatherSource for FixtureWeather {
        async fn forecast(
            &self,
            _lat: f64,
            _lon: f64,
            _start: chrono::DateTime<chrono::Utc>,
            _end: chrono::DateTime<chrono::Utc>,
        ) -> Result<RawForecast, String> {
            self.result.clone()
        }
    }

    /// Unwrap a `VersionedHike` known to be V2 for assertions.
    fn v2(h: VersionedHike) -> HikeResponseV2 {
        match h {
            VersionedHike::V2(r) => r,
            VersionedHike::V1(_) => panic!("expected v2 response"),
        }
    }

    fn sample_record() -> HikeRecord {
        HikeRecord {
            id: "blue-ridge".to_string(),
            start: "2026-07-18T08:00:00-04:00".to_string(),
            end: "2026-07-18T12:00:00-04:00".to_string(),
            meeting: MeetingCoords {
                lat: 37.6,
                lon: -79.2,
            },
            trails: vec!["Blue Ridge Loop".to_string()],
            map_key: "hikes/blue-ridge/map.png".to_string(),
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

    // @spec API-RESP-004
    #[tokio::test]
    async fn missing_hike_returns_none() {
        let store = FixtureStore { record: None };
        let weather = FixtureWeather {
            result: Ok(RawForecast::default()),
        };
        let result = build_hike_response(&store, &weather, "nope", ApiVersion::V2)
            .await
            .unwrap();
        assert!(result.is_none());
    }

    // @spec API-RESP-005, API-RESP-006
    #[tokio::test]
    async fn found_hike_with_weather_failure_is_best_effort() {
        let store = FixtureStore {
            record: Some(sample_record()),
        };
        let weather = FixtureWeather {
            result: Err("nws down".to_string()),
        };
        let response = v2(build_hike_response(&store, &weather, "x", ApiVersion::V2)
            .await
            .unwrap()
            .unwrap());
        assert!(!response.weather_available);
        assert!(response.weather.is_none());
        assert_eq!(response.id, "blue-ridge");
    }

    // @spec API-RESP-006
    #[tokio::test]
    async fn a_forecast_populates_weather() {
        let store = FixtureStore {
            record: Some(sample_record()),
        };
        let weather = FixtureWeather {
            result: Ok(sample_forecast()),
        };
        let response = v2(build_hike_response(&store, &weather, "x", ApiVersion::V2)
            .await
            .unwrap()
            .unwrap());
        assert!(response.weather_available);
        assert_eq!(response.weather.unwrap().conditions, "Partly Cloudy");
    }

    /// The record's timestamps reach the response untouched — offset included —
    /// so the app renders local time without knowing the preserve's timezone.
    // @spec API-RESP-002
    #[tokio::test]
    async fn start_and_end_are_echoed_verbatim_with_their_offset() {
        let store = FixtureStore {
            record: Some(sample_record()),
        };
        let weather = FixtureWeather {
            result: Ok(sample_forecast()),
        };
        let response = v2(build_hike_response(&store, &weather, "x", ApiVersion::V2)
            .await
            .unwrap()
            .unwrap());
        assert_eq!(response.start, "2026-07-18T08:00:00-04:00");
        assert_eq!(response.end, "2026-07-18T12:00:00-04:00");
    }

    /// The client gets a ready-to-open maps link rather than composing one.
    // @spec API-RESP-003
    #[tokio::test]
    async fn meeting_point_carries_a_google_maps_url() {
        let store = FixtureStore {
            record: Some(sample_record()),
        };
        let weather = FixtureWeather {
            result: Ok(sample_forecast()),
        };
        let response = v2(build_hike_response(&store, &weather, "x", ApiVersion::V2)
            .await
            .unwrap()
            .unwrap());
        assert_eq!(response.meeting_point.lat, 37.6);
        assert_eq!(response.meeting_point.lon, -79.2);
        assert_eq!(
            response.meeting_point.google_maps_url,
            "https://maps.google.com/?q=37.6,-79.2"
        );
    }

    /// Every envelope field a hike screen needs is present in one response.
    // @spec API-RESP-001
    #[tokio::test]
    async fn a_found_hike_returns_the_full_envelope() {
        let store = FixtureStore {
            record: Some(sample_record()),
        };
        let weather = FixtureWeather {
            result: Ok(sample_forecast()),
        };
        let response = v2(build_hike_response(&store, &weather, "x", ApiVersion::V2)
            .await
            .unwrap()
            .unwrap());
        assert_eq!(response.id, "blue-ridge");
        assert_eq!(response.trails, vec!["Blue Ridge Loop".to_string()]);
        assert_eq!(response.map.url, "https://example.com/map.png");
        assert!(!response.map.expires_at.is_empty());
        assert!(response.weather_available);
    }

    /// Storage failure is not weather: it ends the request rather than degrading,
    /// because the core of a hike screen cannot be assembled without it.
    /// A record still carrying the upload template's placeholder dates must fail
    /// loudly rather than surfacing as a hike with mysteriously no weather.
    // @spec HIKE-REC-006
    #[tokio::test]
    async fn an_unparseable_start_fails_the_request() {
        let mut record = sample_record();
        record.start = "TODO".to_string();
        let store = FixtureStore {
            record: Some(record),
        };
        let weather = FixtureWeather {
            result: Ok(sample_forecast()),
        };
        let Err(err) = build_hike_response(&store, &weather, "x", ApiVersion::V2).await else {
            panic!("expected an unparseable start to fail the request");
        };
        assert!(!err.is_empty());
    }

    /// The record's offset has to reach the v2 builder, which reports precip
    /// timing in it — a UTC-normalised instant would lose the hiker's day.
    // @spec API-RESP-008
    #[tokio::test]
    async fn the_records_offset_reaches_the_v2_weather_block() {
        let store = FixtureStore {
            record: Some(sample_record()),
        };
        let mut forecast = sample_forecast();
        forecast.periods[0].precip_prob_pct = 80;
        let weather = FixtureWeather {
            result: Ok(forecast),
        };
        let response = build_hike_response(&store, &weather, "x", ApiVersion::V2)
            .await
            .unwrap()
            .unwrap();
        match response {
            VersionedHike::V2(r) => {
                let starts_at = r.weather.unwrap().precipitation.starts_at.unwrap();
                assert!(
                    starts_at.ends_with("-04:00"),
                    "expected the record's offset, got {starts_at}"
                );
            }
            VersionedHike::V1(_) => panic!("expected v2 response"),
        }
    }

    // @spec API-RESP-007
    #[tokio::test]
    async fn store_failure_fails_the_request_rather_than_degrading() {
        let weather = FixtureWeather {
            result: Ok(sample_forecast()),
        };
        let result = build_hike_response(&FailingStore, &weather, "x", ApiVersion::V2).await;
        let Err(err) = result else {
            panic!("expected storage failure to fail the request");
        };
        assert_eq!(err, "r2 unavailable");
    }

    // @spec API-WIRE-003, WX-OUT-003
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

    /// v2's shape has no way to say "no map", so a missing map object fails the
    /// request rather than returning a map URL that points at nothing.
    // @spec API-RESP-011, HIKE-MAP-009
    #[tokio::test]
    async fn v2_fails_the_request_when_the_map_object_is_missing() {
        let weather = FixtureWeather {
            result: Ok(sample_forecast()),
        };
        let Err(err) = build_hike_response(&MaplessStore, &weather, "x", ApiVersion::V2).await
        else {
            panic!("expected a missing map to fail a v2 request");
        };
        assert_eq!(err, "map object not found: hikes/blue-ridge/map.png");
    }

    /// v3 is the current version and is served; its own weather and map
    /// shapes are API-WIRE-004 and API-WIRE-009.
    // @spec API-VER-008
    #[tokio::test]
    async fn v3_is_served() {
        let store = FixtureStore {
            record: Some(sample_record()),
        };
        let weather = FixtureWeather {
            result: Ok(sample_forecast()),
        };
        let response = build_hike_response(&store, &weather, "x", ApiVersion::V3)
            .await
            .unwrap();
        assert!(response.is_some());
    }
}
