//! Schema-conformance contract test: proves the Rust response types serialize
//! into shapes that validate against `openapi.yaml`. Runs in-process, no
//! network, no deployed worker — see the plan's CI section for why the
//! full-runtime contract exercise is left to the post-deploy smoke test instead.

use hike_club_api::models::{
    Alert, HikeLocation, HikeResponseV3, MapRef, MeetingPoint, PrecipitationV2, WeatherV3,
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

/// OpenAPI's component schemas as draft-07 `definitions`.
fn definitions() -> serde_json::Value {
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
    definitions
}

fn validator_for(root: &str) -> jsonschema::Validator {
    let schema = serde_json::json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$ref": format!("#/definitions/{root}"),
        "definitions": definitions(),
    });

    jsonschema::validator_for(&schema).expect("openapi.yaml schema must compile")
}

/// A validator for a JSON array whose items are the named component schema.
fn array_validator_for(item: &str) -> jsonschema::Validator {
    let schema = serde_json::json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "type": "array",
        "items": { "$ref": format!("#/definitions/{item}") },
        "definitions": definitions(),
    });
    jsonschema::validator_for(&schema).expect("openapi.yaml schema must compile")
}

fn sample_response_v3(weather: Option<WeatherV3>) -> HikeResponseV3 {
    HikeResponseV3 {
        id: "blue-ridge".to_string(),
        meeting_point: MeetingPoint::new(37.6, -79.2),
        trails: vec!["Blue Ridge Loop".to_string()],
        map: Some(MapRef {
            url: "https://example.r2.cloudflarestorage.com/map.png?sig=abc".to_string(),
            expires_at: "2026-07-18T09:00:00Z".to_string(),
        }),
        map_available: true,
        weather_available: weather.is_some(),
        weather,
    }
}

fn sample_weather_v3() -> WeatherV3 {
    WeatherV3 {
        start_temp_f: 72.0,
        end_temp_f: 81.0,
        start_conditions: "Sunny".to_string(),
        end_conditions: "Thunderstorms".to_string(),
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

// @spec API-WIRE-007, WX-OUT-007
#[test]
fn precip_timing_nulls_validate() {
    // expected=false with null timestamps must still satisfy the schema.
    let validator = validator_for("HikeResponseV3");
    let mut w = sample_weather_v3();
    w.precipitation = PrecipitationV2 {
        probability_pct: 0,
        expected: false,
        starts_at: None,
        ends_at: None,
    };
    let instance = serde_json::to_value(sample_response_v3(Some(w))).unwrap();
    let errors: Vec<_> = validator.iter_errors(&instance).collect();
    assert!(errors.is_empty(), "schema violations: {errors:?}");
}

/// `GET /hike-locations` serializes the parsed list, so its bytes carry only
/// the two fields the spec names; a third field must fail the schema.
// @spec API-LOC-001
#[test]
fn location_list_validates_against_spec() {
    let validator = array_validator_for("HikeLocation");
    let list = vec![HikeLocation {
        short_name: "blackwell-forest-preserve".to_string(),
        full_name: "Blackwell".to_string(),
    }];
    let instance = serde_json::to_value(&list).unwrap();
    let errors: Vec<_> = validator.iter_errors(&instance).collect();
    assert!(errors.is_empty(), "schema violations: {errors:?}");

    let extra = serde_json::json!([{ "short_name": "a", "full_name": "A", "note": "x" }]);
    assert!(
        !validator.is_valid(&extra),
        "the spec must refuse fields beyond short_name and full_name"
    );
}

/// The spec documents both location-list failures and names the slug by its
/// wire field.
// @spec API-LOC-008
#[test]
fn spec_documents_location_list_failures_and_slug_field() {
    let openapi: serde_json::Value =
        serde_json::to_value(serde_yaml::from_str::<serde_yaml::Value>(OPENAPI_YAML).unwrap())
            .unwrap();
    let responses = &openapi["paths"]["/hike-locations"]["get"]["responses"];
    for status in ["500", "502"] {
        assert!(
            responses[status].is_object(),
            "/hike-locations must document {status}"
        );
    }
    assert!(
        !OPENAPI_YAML.contains("shortName"),
        "the slug's wire field is short_name"
    );
}

/// The published document describes the v3 shape — no `start`/`end`, a
/// nullable map beside `mapAvailable`, and conditions at both ends of the
/// window — and no longer carries any schema for the sunset v1 or v2.
// @spec API-WIRE-008, API-WIRE-011
#[test]
fn openapi_publishes_v3_and_drops_sunset_versions() {
    let openapi: serde_yaml::Value = serde_yaml::from_str(OPENAPI_YAML).unwrap();
    let schemas = &openapi["components"]["schemas"];
    for sunset in [
        "HikeResponse",
        "Weather",
        "Precipitation",
        "HikeResponseV2",
        "WeatherV2",
    ] {
        assert!(
            schemas.get(sunset).is_none(),
            "sunset-version schema {sunset} still published"
        );
    }

    let validator = validator_for("HikeResponseV3");
    let instance = serde_json::json!({
        "id": "blue-ridge",
        "meetingPoint": { "lat": 37.6, "lon": -79.2, "googleMapsUrl": "https://maps.google.com/?q=37.6,-79.2" },
        "trails": ["Blue Ridge Loop"],
        "map": null,
        "mapAvailable": false,
        "weatherAvailable": true,
        "weather": {
            "startTempF": 62.0,
            "endTempF": 81.0,
            "startConditions": "Sunny",
            "endConditions": "Thunderstorms",
            "precipitation": { "probabilityPct": 60, "expected": true, "startsAt": null, "endsAt": null },
            "heatIndexF": null,
            "windChillF": null,
            "alerts": []
        }
    });
    let errors: Vec<_> = validator.iter_errors(&instance).collect();
    assert!(errors.is_empty(), "schema violations: {errors:?}");
    assert!(
        !schemas["HikeResponseV3"]["required"]
            .as_sequence()
            .unwrap()
            .iter()
            .any(|f| f == "start" || f == "end"),
        "HikeResponseV3 must not require start/end"
    );

    let mut without_end_conditions = instance.clone();
    without_end_conditions["weather"]
        .as_object_mut()
        .unwrap()
        .remove("endConditions");
    assert!(
        !validator.is_valid(&without_end_conditions),
        "endConditions must be required"
    );
}

/// The weather block pairs conditions with the temperatures at both ends of
/// the window, and carries no bare `conditions` field.
// @spec API-WIRE-004, API-WIRE-007
#[test]
fn weather_block_matches_spec() {
    let validator = validator_for("WeatherV3");
    let w = sample_weather_v3();
    let instance = serde_json::to_value(&w).unwrap();
    let errors: Vec<_> = validator.iter_errors(&instance).collect();
    assert!(errors.is_empty(), "schema violations: {errors:?}");
    assert!(instance.get("conditions").is_none());
}

// @spec API-WIRE-001, API-WIRE-009, API-WIRE-007
#[test]
fn v3_response_with_weather_matches_spec() {
    let validator = validator_for("HikeResponseV3");
    let instance = serde_json::to_value(sample_response_v3(Some(sample_weather_v3()))).unwrap();
    let errors: Vec<_> = validator.iter_errors(&instance).collect();
    assert!(errors.is_empty(), "schema violations: {errors:?}");
    assert_eq!(instance["mapAvailable"], true);
}

// @spec API-RESP-005, API-WIRE-007
#[test]
fn v3_response_without_weather_matches_spec() {
    let validator = validator_for("HikeResponseV3");
    let instance = serde_json::to_value(sample_response_v3(None)).unwrap();
    let errors: Vec<_> = validator.iter_errors(&instance).collect();
    assert!(errors.is_empty(), "schema violations: {errors:?}");
}

/// An absent map serializes as `null` beside `mapAvailable: false`, which the
/// published schema accepts.
// @spec API-RESP-010, API-WIRE-007
#[test]
fn v3_response_without_map_matches_spec() {
    let validator = validator_for("HikeResponseV3");
    let mut r = sample_response_v3(Some(sample_weather_v3()));
    r.map = None;
    r.map_available = false;
    let instance = serde_json::to_value(r).unwrap();
    let errors: Vec<_> = validator.iter_errors(&instance).collect();
    assert!(errors.is_empty(), "schema violations: {errors:?}");
    assert!(instance["map"].is_null());
}
