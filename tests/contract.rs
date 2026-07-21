//! Schema-conformance contract test: proves the Rust response types serialize
//! into shapes that validate against `openapi.yaml`. Runs in-process, no
//! network, no deployed worker — see the plan's CI section for why the
//! full-runtime contract exercise is left to the post-deploy smoke test instead.

use hike_club_api::models::{
    Alert, HikeResponse, HikeResponseV2, MapRef, MeetingPoint, Precipitation, PrecipitationV2,
    Weather, WeatherV2,
};

const OPENAPI_YAML: &str = include_str!("../openapi.yaml");

/// OpenAPI 3.0's `nullable: true` isn't a JSON Schema keyword a generic
/// validator understands. Translate it the way OpenAPI tooling does: a
/// `type: X` becomes `anyOf: [{type: "null"}, {type: X, ...rest}]`, and a
/// `$ref`/`allOf` wrapper becomes `anyOf: [{type: "null"}, {allOf: [...]}]`.
fn desugar_nullable(value: &mut serde_json::Value) {
    if let Some(obj) = value.as_object_mut() {
        for v in obj.values_mut() {
            desugar_nullable(v);
        }
        if obj.remove("nullable").is_some() {
            let rest = std::mem::take(obj);
            obj.insert(
                "anyOf".to_string(),
                serde_json::json!([{ "type": "null" }, serde_json::Value::Object(rest)]),
            );
        }
    } else if let Some(arr) = value.as_array_mut() {
        for v in arr.iter_mut() {
            desugar_nullable(v);
        }
    }
}

fn validator_for(root: &str) -> jsonschema::Validator {
    let openapi: serde_json::Value =
        serde_json::to_value(serde_yaml::from_str::<serde_yaml::Value>(OPENAPI_YAML).unwrap())
            .unwrap();
    let schemas = openapi["components"]["schemas"].clone();

    // jsonschema needs a self-contained draft-07 document; rewrite OpenAPI's
    // "#/components/schemas/X" refs to plain "#/definitions/X" and nest the
    // component schemas there.
    let rewritten = schemas
        .to_string()
        .replace("#/components/schemas/", "#/definitions/");
    let mut definitions: serde_json::Value = serde_json::from_str(&rewritten).unwrap();
    desugar_nullable(&mut definitions);

    let schema = serde_json::json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$ref": format!("#/definitions/{root}"),
        "definitions": definitions,
    });

    jsonschema::validator_for(&schema).expect("openapi.yaml schema must compile")
}

fn sample_response(weather: Option<Weather>) -> HikeResponse {
    HikeResponse {
        id: "2026-07-18-blue-ridge".to_string(),
        start: "2026-07-18T08:00:00-04:00".to_string(),
        end: "2026-07-18T12:00:00-04:00".to_string(),
        meeting_point: MeetingPoint::new(37.6, -79.2),
        trails: vec!["Blue Ridge Loop".to_string()],
        map: MapRef {
            url: "https://example.r2.cloudflarestorage.com/map.png?sig=abc".to_string(),
            expires_at: "2026-07-18T09:00:00Z".to_string(),
        },
        weather_available: weather.is_some(),
        weather,
    }
}

fn sample_weather() -> Weather {
    Weather {
        temperature_f: 78.0,
        conditions: "Partly Cloudy".to_string(),
        precipitation: Precipitation {
            probability_pct: 40,
            amount_in: 0.0,
        },
        heat_index_f: Some(82.0),
        wind_chill_f: None,
        alerts: vec![Alert {
            kind: "precip".to_string(),
            message: "Rain likely 10-11am".to_string(),
        }],
    }
}

#[test]
fn response_with_weather_matches_spec() {
    let validator = validator_for("HikeResponse");
    let instance = serde_json::to_value(sample_response(Some(sample_weather()))).unwrap();
    let errors: Vec<_> = validator.iter_errors(&instance).collect();
    assert!(errors.is_empty(), "schema violations: {errors:?}");
}

#[test]
fn response_without_weather_matches_spec() {
    let validator = validator_for("HikeResponse");
    let instance = serde_json::to_value(sample_response(None)).unwrap();
    let errors: Vec<_> = validator.iter_errors(&instance).collect();
    assert!(errors.is_empty(), "schema violations: {errors:?}");
}

fn sample_response_v2(weather: Option<WeatherV2>) -> HikeResponseV2 {
    HikeResponseV2 {
        id: "2026-07-18-blue-ridge".to_string(),
        start: "2026-07-18T08:00:00-04:00".to_string(),
        end: "2026-07-18T12:00:00-04:00".to_string(),
        meeting_point: MeetingPoint::new(37.6, -79.2),
        trails: vec!["Blue Ridge Loop".to_string()],
        map: MapRef {
            url: "https://example.r2.cloudflarestorage.com/map.png?sig=abc".to_string(),
            expires_at: "2026-07-18T09:00:00Z".to_string(),
        },
        weather_available: weather.is_some(),
        weather,
    }
}

fn sample_weather_v2() -> WeatherV2 {
    WeatherV2 {
        start_temp_f: 72.0,
        end_temp_f: 81.0,
        conditions: "Partly Cloudy".to_string(),
        precipitation: PrecipitationV2 {
            probability_pct: 40,
            expected: true,
            starts_at: Some("2026-07-18T06:00:00-04:00".to_string()),
            ends_at: Some("2026-07-18T09:00:00-04:00".to_string()),
        },
        heat_index_f: Some(85.0),
        wind_chill_f: None,
        alerts: vec![Alert {
            kind: "nws_alert".to_string(),
            message: "Flash Flood Watch".to_string(),
        }],
    }
}

#[test]
fn v2_response_with_weather_matches_spec() {
    let validator = validator_for("HikeResponseV2");
    let instance = serde_json::to_value(sample_response_v2(Some(sample_weather_v2()))).unwrap();
    let errors: Vec<_> = validator.iter_errors(&instance).collect();
    assert!(errors.is_empty(), "schema violations: {errors:?}");
}

#[test]
fn v2_response_without_weather_matches_spec() {
    let validator = validator_for("HikeResponseV2");
    let instance = serde_json::to_value(sample_response_v2(None)).unwrap();
    let errors: Vec<_> = validator.iter_errors(&instance).collect();
    assert!(errors.is_empty(), "schema violations: {errors:?}");
}

#[test]
fn v2_precip_timing_nulls_validate() {
    // expected=false with null timestamps must still satisfy the schema.
    let validator = validator_for("HikeResponseV2");
    let mut w = sample_weather_v2();
    w.precipitation = PrecipitationV2 {
        probability_pct: 0,
        expected: false,
        starts_at: None,
        ends_at: None,
    };
    let instance = serde_json::to_value(sample_response_v2(Some(w))).unwrap();
    let errors: Vec<_> = validator.iter_errors(&instance).collect();
    assert!(errors.is_empty(), "schema violations: {errors:?}");
}

/// `GET /hike-locations` serves this file verbatim, so nothing else validates
/// its shape. Guard that it stays a non-empty array of {short_name, full_name}.
#[test]
fn hike_location_mapping_is_well_formed() {
    const MAPPING_JSON: &str = include_str!("../resources/hike-location-mapping.json");
    let entries: Vec<serde_json::Value> =
        serde_json::from_str(MAPPING_JSON).expect("mapping must be a JSON array");
    assert!(!entries.is_empty(), "mapping must not be empty");
    for entry in &entries {
        assert!(
            entry["short_name"].is_string() && entry["full_name"].is_string(),
            "each entry needs string short_name and full_name: {entry}"
        );
    }
}
