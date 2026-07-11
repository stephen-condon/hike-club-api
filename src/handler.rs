use crate::models::{HikeResponse, MapRef, MeetingPoint};
use crate::r2::HikeStore;
use crate::weather::{WeatherSource, build_weather};

/// Pure orchestration: fetch hike metadata + weather, assemble the response.
/// Generic over both traits so tests can inject fixtures with zero network.
pub async fn build_hike_response<S: HikeStore, W: WeatherSource>(
    store: &S,
    weather_source: &W,
    id: &str,
) -> Result<Option<HikeResponse>, String> {
    let Some(record) = store.get_hike(id).await? else {
        return Ok(None);
    };

    let (map_url, expires_at) = store.presign_map_url(&record.map_key).await?;

    let start: chrono::DateTime<chrono::Utc> = record
        .start
        .parse()
        .map_err(|e: chrono::ParseError| e.to_string())?;
    let end: chrono::DateTime<chrono::Utc> = record
        .end
        .parse()
        .map_err(|e: chrono::ParseError| e.to_string())?;

    let forecast = weather_source
        .forecast(record.meeting.lat, record.meeting.lon, start, end)
        .await;

    let (weather_available, weather) = match forecast {
        Ok(raw) => {
            let weather = build_weather(&raw);
            (weather.is_some(), weather)
        }
        Err(_) => (false, None),
    };

    Ok(Some(HikeResponse {
        id: record.id,
        start: record.start,
        end: record.end,
        meeting_point: MeetingPoint::new(record.meeting.lat, record.meeting.lon),
        trails: record.trails,
        map: MapRef {
            url: map_url,
            expires_at: expires_at.to_rfc3339(),
        },
        weather_available,
        weather,
    }))
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

    #[tokio::test]
    async fn missing_hike_returns_none() {
        let store = FixtureStore { record: None };
        let weather = FixtureWeather {
            result: Ok(RawForecast::default()),
        };
        let result = build_hike_response(&store, &weather, "nope").await.unwrap();
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
        let response = build_hike_response(&store, &weather, "x")
            .await
            .unwrap()
            .unwrap();
        assert!(!response.weather_available);
        assert!(response.weather.is_none());
        assert_eq!(response.id, "2026-07-18-blue-ridge");
    }

    #[tokio::test]
    async fn found_hike_with_weather_populates_response() {
        let store = FixtureStore {
            record: Some(sample_record()),
        };
        let raw = RawForecast {
            periods: vec![crate::weather::RawPeriod {
                start: "2026-07-18T12:00:00Z".parse().unwrap(),
                end: "2026-07-18T13:00:00Z".parse().unwrap(),
                temp_f: 78.0,
                humidity_pct: Some(50.0),
                wind_mph: Some(5.0),
                precip_prob_pct: 10,
                short_forecast: "Partly Cloudy".to_string(),
            }],
            active_alerts: vec![],
        };
        let weather = FixtureWeather { result: Ok(raw) };
        let response = build_hike_response(&store, &weather, "x")
            .await
            .unwrap()
            .unwrap();
        assert!(response.weather_available);
        assert_eq!(response.weather.unwrap().conditions, "Partly Cloudy");
    }
}
