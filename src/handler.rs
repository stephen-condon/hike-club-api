use crate::models::{HikeResponseV2, HikeResponseV3, MapRef, MeetingPoint};
use crate::r2::HikeStore;
use crate::version::ApiVersion;
use crate::weather::{WeatherSource, build_weather_v2, build_weather_v3};
use chrono::{DateTime, FixedOffset};

/// A hike's window, as supplied per-request. Used to build the record's
/// weather query and, under API version 3, is the only source of `start`/`end`
/// — the record itself may carry none.
pub type Window = (DateTime<FixedOffset>, DateTime<FixedOffset>);

/// Parses the `start`/`end` query parameters `GET /hike/{id}` accepts under API
/// version 3. Version 2 has its own window (the record's `start`/`end`) and
/// ignores the query entirely, so this returns `Ok(None)` for every version but
/// V3 without inspecting `start`/`end` at all.
// @spec API-WIN-001, API-WIN-002, API-WIN-003
pub fn parse_window(
    version: ApiVersion,
    start: Option<&str>,
    end: Option<&str>,
) -> Result<Option<Window>, String> {
    if version != ApiVersion::V3 {
        return Ok(None);
    }
    let start = start.ok_or_else(|| "start query parameter is required".to_string())?;
    let end = end.ok_or_else(|| "end query parameter is required".to_string())?;
    let start = DateTime::parse_from_rfc3339(start)
        .map_err(|_| "start is not a valid RFC 3339 timestamp".to_string())?;
    let end = DateTime::parse_from_rfc3339(end)
        .map_err(|_| "end is not a valid RFC 3339 timestamp".to_string())?;
    if end <= start {
        return Err("end must be after start".to_string());
    }
    Ok(Some((start, end)))
}

/// A hike response in the shape for the requested API version. `lib.rs` matches
/// on this and serializes the appropriate variant.
pub enum VersionedHike {
    V2(HikeResponseV2),
    V3(HikeResponseV3),
}

/// `GET /hike-locations` past admission, as status and body. The list comes
/// only from the store: there is no compiled-in copy to fall back on.
// @spec API-LOC-001, API-LOC-005, API-LOC-006, API-LOC-007
pub async fn build_locations_response<S: HikeStore>(store: &S) -> (u16, String) {
    match store.get_locations().await {
        // A list of plain strings has nothing that can fail to serialize.
        Ok(Some(list)) => (
            200,
            serde_json::to_string(&list).expect("strings serialize"),
        ),
        Ok(None) => (
            500,
            "server misconfigured: location list not found".to_string(),
        ),
        Err(e) => (502, format!("upstream error: {e}")),
    }
}

/// Pure orchestration: fetch hike metadata + the full forecast, assemble the
/// version-appropriate response. Generic over both traits so tests inject
/// fixtures with zero network.
///
/// `window` is the caller-supplied `start`/`end` (`parse_window`, already
/// validated). Version 3 uses it exclusively — the record itself may carry no
/// date. Version 2 ignores it and reads the record's own `start`/`end`, so a
/// dateless record fails a version-2 request.
// @spec API-RESP-001, API-RESP-002, API-RESP-003, API-RESP-004, API-RESP-005,
// @spec API-RESP-006, API-RESP-007, API-RESP-008, API-RESP-009, API-RESP-010,
// @spec API-RESP-011,
// @spec API-WIRE-004, API-WIRE-009, API-WIRE-010, API-WIN-004, API-WIN-005
pub async fn build_hike_response<S: HikeStore, W: WeatherSource>(
    store: &S,
    weather_source: &W,
    id: &str,
    version: ApiVersion,
    window: Option<Window>,
) -> Result<Option<VersionedHike>, String> {
    let Some(record) = store.get_hike(id).await? else {
        return Ok(None);
    };

    // An absent map object is not a failure here; each version decides below
    // whether its shape can say so.
    let map = store
        .presign_map_url(&record.map_key)
        .await?
        .map(|(url, expires_at)| MapRef {
            url,
            expires_at: expires_at.to_rfc3339(),
        });

    let meeting_point = MeetingPoint::new(record.meeting.lat, record.meeting.lon);

    let response = match version {
        // Sunset versions answer 410 before reaching here; one that slips through
        // on an unreadable clock has no shape to borrow.
        ApiVersion::V1 => return Err("api version 1 has no response shape".to_string()),
        ApiVersion::V2 => {
            // v2's shape cannot express an absent map, so it fails the request.
            let map = map.ok_or_else(|| format!("map object not found: {}", record.map_key))?;
            // v2 has no window of its own: the record's start/end — already
            // parsed and validated during retrieval (HIKE-REC-008, HIKE-REC-009)
            // — are the only source, so a dateless record (written for v3 only)
            // fails here.
            let record_start = record
                .start
                .ok_or_else(|| "hike record has no start".to_string())?;
            let record_end = record
                .end
                .ok_or_else(|| "hike record has no end".to_string())?;
            let offset = record_start.timezone();
            let start = record_start.to_utc();
            let end = record_end.to_utc();

            let forecast = weather_source
                .forecast(record.meeting.lat, record.meeting.lon, start, end, offset)
                .await;
            let weather = forecast
                .as_ref()
                .ok()
                .and_then(|raw| build_weather_v2(raw, start, end, offset));
            VersionedHike::V2(HikeResponseV2 {
                id: record.id,
                start: record_start.to_rfc3339(),
                end: record_end.to_rfc3339(),
                meeting_point,
                trails: record.trails,
                map,
                weather_available: weather.is_some(),
                weather,
            })
        }
        ApiVersion::V3 => {
            let (start_local, end_local) =
                window.ok_or_else(|| "start and end query parameters are required".to_string())?;
            let offset = start_local.timezone();
            let start = start_local.to_utc();
            let end = end_local.to_utc();

            let forecast = weather_source
                .forecast(record.meeting.lat, record.meeting.lon, start, end, offset)
                .await;
            let weather = forecast
                .as_ref()
                .ok()
                .and_then(|raw| build_weather_v3(raw, start, end, offset));
            VersionedHike::V3(HikeResponseV3 {
                id: record.id,
                meeting_point,
                trails: record.trails,
                map_available: map.is_some(),
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
    use crate::models::{HikeLocation, HikeRecord, MeetingCoords};
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

        async fn get_locations(&self) -> Result<Option<Vec<HikeLocation>>, String> {
            unreachable!("the hike path reads no location list")
        }
    }

    /// A store whose reads fail, for the path that must not degrade.
    struct FailingStore;

    /// A store holding only a location-list outcome.
    struct LocationsStore(Result<Option<Vec<HikeLocation>>, String>);

    impl HikeStore for LocationsStore {
        async fn get_hike(&self, _id: &str) -> Result<Option<HikeRecord>, String> {
            unreachable!("the locations path reads no hike")
        }

        async fn presign_map_url(
            &self,
            _map_key: &str,
        ) -> Result<Option<(String, chrono::DateTime<chrono::Utc>)>, String> {
            unreachable!("the locations path presigns nothing")
        }

        async fn get_locations(&self) -> Result<Option<Vec<HikeLocation>>, String> {
            self.0.clone()
        }
    }

    fn location(short: &str, full: &str) -> HikeLocation {
        HikeLocation {
            short_name: short.to_string(),
            full_name: full.to_string(),
        }
    }

    // @spec API-LOC-001
    #[tokio::test]
    async fn locations_are_served_as_a_json_array_in_stored_order() {
        let store = LocationsStore(Ok(Some(vec![
            location("oakhurst", "Oakhurst"),
            location("blackwell", "Blackwell"),
        ])));
        assert_eq!(
            build_locations_response(&store).await,
            (
                200,
                r#"[{"short_name":"oakhurst","full_name":"Oakhurst"},{"short_name":"blackwell","full_name":"Blackwell"}]"#
                    .to_string()
            )
        );
    }

    // @spec API-LOC-001, HIKE-LOC-005
    #[tokio::test]
    async fn an_empty_location_list_is_served() {
        let store = LocationsStore(Ok(Some(vec![])));
        assert_eq!(
            build_locations_response(&store).await,
            (200, "[]".to_string())
        );
    }

    // @spec API-LOC-005
    #[tokio::test]
    async fn an_absent_location_list_is_misconfiguration() {
        let store = LocationsStore(Ok(None));
        assert_eq!(
            build_locations_response(&store).await,
            (
                500,
                "server misconfigured: location list not found".to_string()
            )
        );
    }

    // @spec API-LOC-006, API-LOC-007
    #[tokio::test]
    async fn an_unreadable_location_list_is_an_upstream_error() {
        let store = LocationsStore(Err("bad json".to_string()));
        assert_eq!(
            build_locations_response(&store).await,
            (502, "upstream error: bad json".to_string())
        );
    }

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

        async fn get_locations(&self) -> Result<Option<Vec<HikeLocation>>, String> {
            unreachable!("the hike path reads no location list")
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

        async fn get_locations(&self) -> Result<Option<Vec<HikeLocation>>, String> {
            unreachable!("the hike path reads no location list")
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
            _offset: FixedOffset,
        ) -> Result<RawForecast, String> {
            self.result.clone()
        }
    }

    /// A `WeatherSource` that records the offset it was called with, so tests
    /// can assert what `build_hike_response` passed through.
    struct SpyWeather {
        result: Result<RawForecast, String>,
        seen_offset: std::cell::Cell<Option<FixedOffset>>,
    }

    impl WeatherSource for SpyWeather {
        async fn forecast(
            &self,
            _lat: f64,
            _lon: f64,
            _start: chrono::DateTime<chrono::Utc>,
            _end: chrono::DateTime<chrono::Utc>,
            offset: FixedOffset,
        ) -> Result<RawForecast, String> {
            self.seen_offset.set(Some(offset));
            self.result.clone()
        }
    }

    // @spec API-WIN-001
    #[test]
    fn non_v3_ignores_the_query_entirely() {
        // Garbage that would fail to parse if it were ever read.
        assert_eq!(
            parse_window(ApiVersion::V2, Some("not a date"), None),
            Ok(None)
        );
        assert_eq!(parse_window(ApiVersion::V1, None, None), Ok(None));
    }

    // @spec API-WIN-002
    #[test]
    fn v3_requires_both_query_parameters() {
        assert_eq!(
            parse_window(ApiVersion::V3, None, Some("2026-07-18T12:00:00-04:00")),
            Err("start query parameter is required".to_string())
        );
        assert_eq!(
            parse_window(ApiVersion::V3, Some("2026-07-18T08:00:00-04:00"), None),
            Err("end query parameter is required".to_string())
        );
    }

    // @spec API-WIN-002
    #[test]
    fn v3_rejects_a_malformed_timestamp() {
        assert_eq!(
            parse_window(
                ApiVersion::V3,
                Some("not a date"),
                Some("2026-07-18T12:00:00-04:00")
            ),
            Err("start is not a valid RFC 3339 timestamp".to_string())
        );
        assert_eq!(
            parse_window(
                ApiVersion::V3,
                Some("2026-07-18T08:00:00-04:00"),
                Some("not a date")
            ),
            Err("end is not a valid RFC 3339 timestamp".to_string())
        );
    }

    // @spec API-WIN-003
    #[test]
    fn v3_rejects_an_end_not_after_start() {
        let same = "2026-07-18T08:00:00-04:00";
        assert_eq!(
            parse_window(ApiVersion::V3, Some(same), Some(same)),
            Err("end must be after start".to_string())
        );
        assert_eq!(
            parse_window(
                ApiVersion::V3,
                Some("2026-07-18T12:00:00-04:00"),
                Some("2026-07-18T08:00:00-04:00"),
            ),
            Err("end must be after start".to_string())
        );
    }

    // @spec API-WIN-002, API-WIN-003
    #[test]
    fn v3_accepts_a_valid_window() {
        let start = "2026-07-18T08:00:00-04:00";
        let end = "2026-07-18T12:00:00-04:00";
        let window = parse_window(ApiVersion::V3, Some(start), Some(end)).unwrap();
        assert_eq!(
            window,
            Some((
                DateTime::parse_from_rfc3339(start).unwrap(),
                DateTime::parse_from_rfc3339(end).unwrap(),
            ))
        );
    }

    /// Unwrap a `VersionedHike` known to be V2 for assertions.
    fn v2(h: VersionedHike) -> HikeResponseV2 {
        match h {
            VersionedHike::V2(r) => r,
            VersionedHike::V3(_) => panic!("expected the v2 shape"),
        }
    }

    fn sample_record() -> HikeRecord {
        HikeRecord {
            id: "blue-ridge".to_string(),
            start: Some("2026-07-18T08:00:00-04:00".parse().unwrap()),
            end: Some("2026-07-18T12:00:00-04:00".parse().unwrap()),
            meeting: MeetingCoords {
                lat: 37.6,
                lon: -79.2,
            },
            trails: vec!["Blue Ridge Loop".to_string()],
            map_key: "hikes/blue-ridge/map.png".to_string(),
        }
    }

    /// A record written for v3-only service: no start/end at all.
    fn dateless_record() -> HikeRecord {
        HikeRecord {
            start: None,
            end: None,
            ..sample_record()
        }
    }

    /// The window a v3 caller supplies, matching `sample_record`'s dates so
    /// tests that don't care about the window's independence can reuse it.
    fn sample_window() -> Window {
        (
            DateTime::parse_from_rfc3339("2026-07-18T08:00:00-04:00").unwrap(),
            DateTime::parse_from_rfc3339("2026-07-18T12:00:00-04:00").unwrap(),
        )
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
        let result = build_hike_response(&store, &weather, "nope", ApiVersion::V2, None)
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
        let response = v2(
            build_hike_response(&store, &weather, "x", ApiVersion::V2, None)
                .await
                .unwrap()
                .unwrap(),
        );
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
        let response = v2(
            build_hike_response(&store, &weather, "x", ApiVersion::V2, None)
                .await
                .unwrap()
                .unwrap(),
        );
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
        let response = v2(
            build_hike_response(&store, &weather, "x", ApiVersion::V2, None)
                .await
                .unwrap()
                .unwrap(),
        );
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
        let response = v2(
            build_hike_response(&store, &weather, "x", ApiVersion::V2, None)
                .await
                .unwrap()
                .unwrap(),
        );
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
        let response = v2(
            build_hike_response(&store, &weather, "x", ApiVersion::V2, None)
                .await
                .unwrap()
                .unwrap(),
        );
        assert_eq!(response.id, "blue-ridge");
        assert_eq!(response.trails, vec!["Blue Ridge Loop".to_string()]);
        assert_eq!(response.map.url, "https://example.com/map.png");
        assert!(!response.map.expires_at.is_empty());
        assert!(response.weather_available);
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
        let r = v2(
            build_hike_response(&store, &weather, "x", ApiVersion::V2, None)
                .await
                .unwrap()
                .unwrap(),
        );
        let starts_at = r.weather.unwrap().precipitation.starts_at.unwrap();
        assert!(
            starts_at.ends_with("-04:00"),
            "expected the record's offset, got {starts_at}"
        );
    }

    // @spec API-RESP-007
    #[tokio::test]
    async fn store_failure_fails_the_request_rather_than_degrading() {
        let weather = FixtureWeather {
            result: Ok(sample_forecast()),
        };
        let result = build_hike_response(&FailingStore, &weather, "x", ApiVersion::V2, None).await;
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
        let r = v2(
            build_hike_response(&store, &weather, "x", ApiVersion::V2, None)
                .await
                .unwrap()
                .unwrap(),
        );
        let w = r.weather.unwrap();
        assert_eq!(w.start_temp_f, 78.0);
        assert_eq!(w.end_temp_f, 78.0);
    }

    /// v2's shape has no way to say "no map", so a missing map object fails the
    /// request rather than returning a map URL that points at nothing.
    // @spec API-RESP-011, HIKE-MAP-009
    #[tokio::test]
    async fn v2_fails_the_request_when_the_map_object_is_missing() {
        let weather = FixtureWeather {
            result: Ok(sample_forecast()),
        };
        let Err(err) =
            build_hike_response(&MaplessStore, &weather, "x", ApiVersion::V2, None).await
        else {
            panic!("expected a missing map to fail a v2 request");
        };
        assert_eq!(err, "map object not found: hikes/blue-ridge/map.png");
    }

    /// A hike whose map image never uploaded still gets the pack to the
    /// trailhead under v3: the map is absent and flagged, and nothing else is.
    // @spec API-RESP-010, HIKE-MAP-009
    #[tokio::test]
    async fn v3_serves_a_missing_map_as_absent() {
        let weather = FixtureWeather {
            result: Ok(sample_forecast()),
        };
        let response = build_hike_response(
            &MaplessStore,
            &weather,
            "x",
            ApiVersion::V3,
            Some(sample_window()),
        )
        .await
        .unwrap()
        .unwrap();
        let VersionedHike::V3(r) = response else {
            panic!("expected the v3 shape");
        };
        assert!(r.map.is_none());
        assert!(!r.map_available);
        assert_eq!(r.meeting_point.lat, 37.6);
        assert!(r.weather_available);
    }

    /// v3 needs no dates on the record at all: the caller's window is the only
    /// source, so a dateless record — the shape the admin will write once it
    /// stops writing dates — still serves.
    // @spec API-WIN-004, API-WIN-005
    #[tokio::test]
    async fn v3_serves_a_dateless_record_from_the_query_window() {
        let store = FixtureStore {
            record: Some(dateless_record()),
        };
        let weather = FixtureWeather {
            result: Ok(sample_forecast()),
        };
        let response =
            build_hike_response(&store, &weather, "x", ApiVersion::V3, Some(sample_window()))
                .await
                .unwrap()
                .unwrap();
        let VersionedHike::V3(r) = response else {
            panic!("expected the v3 shape");
        };
        assert!(r.weather_available);
    }

    /// The opposite of v3: a record with no start/end has nothing v2 can read,
    /// since v2 has no query window to fall back on.
    // @spec API-WIN-005
    #[tokio::test]
    async fn v2_fails_a_dateless_record() {
        let store = FixtureStore {
            record: Some(dateless_record()),
        };
        let weather = FixtureWeather {
            result: Ok(sample_forecast()),
        };
        let Err(err) = build_hike_response(&store, &weather, "x", ApiVersion::V2, None).await
        else {
            panic!("expected a dateless record to fail a v2 request");
        };
        assert_eq!(err, "hike record has no start");
    }

    /// v3 ignores the record's own dates entirely — even when it has some, the
    /// query window governs the weather query and offset.
    // @spec API-WIN-004
    #[tokio::test]
    async fn v3_uses_the_query_window_not_the_records_dates() {
        // The record's dates would put the hike window in the fall; the query
        // window sent below is the summer one `sample_forecast` covers.
        let mut record = sample_record();
        record.start = Some("2026-11-01T08:00:00-04:00".parse().unwrap());
        record.end = Some("2026-11-01T12:00:00-04:00".parse().unwrap());
        let store = FixtureStore {
            record: Some(record),
        };
        let weather = FixtureWeather {
            result: Ok(sample_forecast()),
        };
        let response =
            build_hike_response(&store, &weather, "x", ApiVersion::V3, Some(sample_window()))
                .await
                .unwrap()
                .unwrap();
        let VersionedHike::V3(r) = response else {
            panic!("expected the v3 shape");
        };
        assert!(r.weather_available, "expected the query window's weather");
    }

    /// v3 with no query window at all — the case a malformed request would
    /// somehow slip through as — fails rather than serving with no dates.
    // @spec API-WIN-004
    #[tokio::test]
    async fn v3_fails_without_a_window() {
        let store = FixtureStore {
            record: Some(sample_record()),
        };
        let weather = FixtureWeather {
            result: Ok(sample_forecast()),
        };
        let Err(err) = build_hike_response(&store, &weather, "x", ApiVersion::V3, None).await
        else {
            panic!("expected a missing window to fail a v3 request");
        };
        assert_eq!(err, "start and end query parameters are required");
    }

    /// A sunset version answers 410 before assembly; if an unreadable clock lets
    /// one through anyway, it fails rather than borrowing another version's shape.
    // @spec API-WIRE-010
    #[tokio::test]
    async fn a_sunset_version_has_no_response_shape() {
        let store = FixtureStore {
            record: Some(sample_record()),
        };
        let weather = FixtureWeather {
            result: Ok(sample_forecast()),
        };
        let Err(err) = build_hike_response(&store, &weather, "x", ApiVersion::V1, None).await
        else {
            panic!("expected v1 to have no response shape");
        };
        assert_eq!(err, "api version 1 has no response shape");
    }

    /// v3, the current version, is served in its own shape: a map beside
    /// `mapAvailable`, and conditions at both ends of the window.
    // @spec API-VER-008, API-WIRE-009, API-WIRE-004
    #[tokio::test]
    async fn v3_is_served_in_its_own_shape() {
        let store = FixtureStore {
            record: Some(sample_record()),
        };
        let weather = FixtureWeather {
            result: Ok(sample_forecast()),
        };
        let response =
            build_hike_response(&store, &weather, "x", ApiVersion::V3, Some(sample_window()))
                .await
                .unwrap()
                .unwrap();
        let VersionedHike::V3(r) = response else {
            panic!("expected v3 to be served in the v3 shape");
        };
        assert!(r.map_available);
        assert_eq!(r.map.unwrap().url, "https://example.com/map.png");
        assert!(r.weather_available);
        let w = r.weather.unwrap();
        assert_eq!(w.start_conditions, "Partly Cloudy");
        assert_eq!(w.end_conditions, "Partly Cloudy");
    }

    /// `WeatherSource` needs the hike's UTC offset to key observations by the
    /// same local calendar day precipitation timing uses (WX-CACHE-003). v2
    /// takes it from the record's own start.
    // @spec WX-CACHE-010
    #[tokio::test]
    async fn v2_passes_the_records_offset_to_the_weather_source() {
        let store = FixtureStore {
            record: Some(sample_record()),
        };
        let weather = SpyWeather {
            result: Ok(sample_forecast()),
            seen_offset: std::cell::Cell::new(None),
        };
        build_hike_response(&store, &weather, "x", ApiVersion::V2, None)
            .await
            .unwrap();
        assert_eq!(
            weather.seen_offset.get(),
            Some(FixedOffset::west_opt(4 * 3600).unwrap())
        );
    }

    /// The same, for v3: the offset comes from the caller's query window, not
    /// the record.
    // @spec WX-CACHE-010
    #[tokio::test]
    async fn v3_passes_the_query_windows_offset_to_the_weather_source() {
        let store = FixtureStore {
            record: Some(sample_record()),
        };
        let weather = SpyWeather {
            result: Ok(sample_forecast()),
            seen_offset: std::cell::Cell::new(None),
        };
        build_hike_response(&store, &weather, "x", ApiVersion::V3, Some(sample_window()))
            .await
            .unwrap();
        assert_eq!(
            weather.seen_offset.get(),
            Some(FixedOffset::west_opt(4 * 3600).unwrap())
        );
    }
}
