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
pub struct Precipitation {
    #[serde(rename = "probabilityPct")]
    pub probability_pct: u8,
    #[serde(rename = "amountIn")]
    pub amount_in: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    #[serde(rename = "type")]
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Weather {
    #[serde(rename = "temperatureF")]
    pub temperature_f: f64,
    pub conditions: String,
    pub precipitation: Precipitation,
    #[serde(rename = "heatIndexF")]
    pub heat_index_f: Option<f64>,
    #[serde(rename = "windChillF")]
    pub wind_chill_f: Option<f64>,
    pub alerts: Vec<Alert>,
}

/// v2 precipitation: probability plus *when* precip is expected across the hike's
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

/// v2 weather block: start/end temps, precip timing, alerts filtered to the hike
/// window. `temperatureF` and the always-zero `amountIn` from v1 are dropped.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeatherV2 {
    #[serde(rename = "startTempF")]
    pub start_temp_f: f64,
    #[serde(rename = "endTempF")]
    pub end_temp_f: f64,
    pub conditions: String,
    pub precipitation: PrecipitationV2,
    #[serde(rename = "heatIndexF")]
    pub heat_index_f: Option<f64>,
    #[serde(rename = "windChillF")]
    pub wind_chill_f: Option<f64>,
    pub alerts: Vec<Alert>,
}

/// Raw hike metadata as stored in R2 at `hikes/{id}.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HikeRecord {
    pub id: String,
    pub start: String,
    pub end: String,
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

/// The full GET /hike/{id} response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HikeResponse {
    pub id: String,
    pub start: String,
    pub end: String,
    #[serde(rename = "meetingPoint")]
    pub meeting_point: MeetingPoint,
    pub trails: Vec<String>,
    pub map: MapRef,
    #[serde(rename = "weatherAvailable")]
    pub weather_available: bool,
    pub weather: Option<Weather>,
}

/// The full GET /hike/{id} response under `x-api-version: 2`. Identical to
/// `HikeResponse` except the weather block is `WeatherV2`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HikeResponseV2 {
    pub id: String,
    pub start: String,
    pub end: String,
    #[serde(rename = "meetingPoint")]
    pub meeting_point: MeetingPoint,
    pub trails: Vec<String>,
    pub map: MapRef,
    #[serde(rename = "weatherAvailable")]
    pub weather_available: bool,
    pub weather: Option<WeatherV2>,
}
