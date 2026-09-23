use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingPoint {
    pub lat: f64,
    pub lon: f64,
    #[serde(rename = "googleMapsUrl")]
    pub google_maps_url: String,
}

impl MeetingPoint {
    pub fn new(lat: f64, lon: f64) -> Self {
        Self {
            lat,
            lon,
            google_maps_url: format!("https://maps.google.com/?q={lat},{lon}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapRef {
    pub url: String,
    #[serde(rename = "expiresAt")]
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    #[serde(rename = "type")]
    pub kind: String,
    pub message: String,
}

/// Precipitation: probability plus *when* precip is expected across the hike's
/// local calendar day (times may fall before/after the hike window).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrecipitationV2 {
    #[serde(rename = "probabilityPct")]
    pub probability_pct: u8,
    /// Whether any hour on the hike's calendar day is at/above the "likely" threshold.
    pub expected: bool,
    /// RFC 3339 start of the first likely-precip hour (in the hike's local offset), if any.
    #[serde(rename = "startsAt")]
    pub starts_at: Option<String>,
    /// RFC 3339 end of the last likely-precip hour (in the hike's local offset), if any.
    #[serde(rename = "endsAt")]
    pub ends_at: Option<String>,
}

/// One entry of the location list stored in R2 at `resources/hike-locations.json`
/// and served by `GET /hike-locations`. Unknown fields are ignored on read and so
/// never reach the response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HikeLocation {
    pub short_name: String,
    pub full_name: String,
}

/// The weather block: temperatures and conditions reported at both ends of the
/// hike window so they read the same way as each other.
// @spec API-WIRE-004
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeatherV3 {
    #[serde(rename = "startTempF")]
    pub start_temp_f: f64,
    #[serde(rename = "endTempF")]
    pub end_temp_f: f64,
    #[serde(rename = "startConditions")]
    pub start_conditions: String,
    #[serde(rename = "endConditions")]
    pub end_conditions: String,
    pub precipitation: PrecipitationV2,
    #[serde(rename = "heatIndexF")]
    pub heat_index_f: Option<f64>,
    #[serde(rename = "windChillF")]
    pub wind_chill_f: Option<f64>,
    pub alerts: Vec<Alert>,
}

/// Raw hike metadata as stored in R2 at `hikes/{id}.json`, where `id` is the
/// location slug (a `short_name` from the location list) with no date
/// component — one record per location, rewritten in place when that location
/// is next scheduled. The record itself carries no date: the caller supplies
/// the hike's window per request (`API-WIN-*`). The admin still writes
/// `start`/`end` on every record for its own UI; this schema does not define
/// them, so they parse as unknown JSON and are dropped (`HIKE-REC-004`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HikeRecord {
    pub id: String,
    pub meeting: MeetingCoords,
    pub trails: Vec<String>,
    #[serde(rename = "mapKey")]
    pub map_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingCoords {
    pub lat: f64,
    pub lon: f64,
}

/// The full GET /hike/{id} response under `x-api-version: 3`. `map` is
/// nullable beside `mapAvailable`, the way `weather` degrades beside
/// `weatherAvailable`. Carries no `start`/`end`: the caller supplies the
/// hike's window as query parameters and the app is the only place that
/// window is stored (`API-WIN-*`).
// @spec API-WIRE-009, API-WIN-004
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HikeResponseV3 {
    pub id: String,
    #[serde(rename = "meetingPoint")]
    pub meeting_point: MeetingPoint,
    pub trails: Vec<String>,
    pub map: Option<MapRef>,
    #[serde(rename = "mapAvailable")]
    pub map_available: bool,
    #[serde(rename = "weatherAvailable")]
    pub weather_available: bool,
    pub weather: Option<WeatherV3>,
}
