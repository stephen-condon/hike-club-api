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
