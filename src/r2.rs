use crate::models::{HikeLocation, HikeRecord};
use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// Abstraction over hike storage so handlers can be unit-tested without R2/network.
/// Real impl (`R2HikeStore`) reads the R2 binding; tests use an in-memory fake.
pub trait HikeStore {
    async fn get_hike(&self, id: &str) -> Result<Option<HikeRecord>, String>;
    /// `Ok(None)` when the object named by `map_key` does not exist.
    async fn presign_map_url(
        &self,
        map_key: &str,
    ) -> Result<Option<(String, DateTime<Utc>)>, String>;
    /// `Ok(None)` when the list object is absent, `Err` when it is unreadable.
    async fn get_locations(&self) -> Result<Option<Vec<HikeLocation>>, String>;
}

/// R2 key of the location list, written by `hike-club-admin`.
pub const LOCATIONS_KEY: &str = "resources/hike-locations.json";

/// Parses the stored location list: shape only, since the admin enforces the
/// content rules when it writes.
// @spec HIKE-LOC-003, HIKE-LOC-004, HIKE-LOC-005, HIKE-LOC-006
pub fn parse_locations(bytes: &[u8]) -> Result<Vec<HikeLocation>, String> {
    serde_json::from_slice(bytes).map_err(|e| e.to_string())
}

/// Parses a stored hike record. The record carries no window of its own — the
/// caller supplies `start`/`end` per request (`API-WIN-*`) — so parsing is
/// the only check: a `start`/`end` the admin still writes for its own UI is
/// unknown JSON to this schema and is dropped like any other field it does
/// not define (`HIKE-REC-004`).
// @spec HIKE-REC-003
pub fn parse_hike_record(bytes: &[u8]) -> Result<HikeRecord, String> {
    serde_json::from_slice(bytes).map_err(|e| e.to_string())
}

pub struct R2Config {
    pub account_id: String,
    pub bucket: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub presign_ttl_secs: u64,
}

/// Builds an R2 (S3-compatible) presigned GET URL via AWS SigV4 query signing.
/// Pure function of `now` so it's unit-testable without a wasm/JS clock.
// @spec HIKE-MAP-003, HIKE-MAP-004, HIKE-MAP-008
pub fn presign_get_url(
    now: DateTime<Utc>,
    account_id: &str,
    bucket: &str,
    object_key: &str,
    access_key_id: &str,
    secret_access_key: &str,
    expires_in_secs: u64,
) -> String {
    let region = "auto";
    let service = "s3";
    let host = format!("{account_id}.r2.cloudflarestorage.com");
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date_stamp = now.format("%Y%m%d").to_string();
    let credential_scope = format!("{date_stamp}/{region}/{service}/aws4_request");
    let credential = uri_encode(&format!("{access_key_id}/{credential_scope}"), true);

    let canonical_uri = format!("/{}/{}", bucket, uri_path_encode(object_key));

    let mut query_pairs = [
        (
            "X-Amz-Algorithm".to_string(),
            "AWS4-HMAC-SHA256".to_string(),
        ),
        ("X-Amz-Credential".to_string(), credential),
        ("X-Amz-Date".to_string(), amz_date.clone()),
        ("X-Amz-Expires".to_string(), expires_in_secs.to_string()),
        ("X-Amz-SignedHeaders".to_string(), "host".to_string()),
    ];
    query_pairs.sort();
    let canonical_query_string = query_pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");

    let canonical_headers = format!("host:{host}\n");
    let signed_headers = "host";
    let canonical_request = format!(
        "GET\n{canonical_uri}\n{canonical_query_string}\n{canonical_headers}\n{signed_headers}\nUNSIGNED-PAYLOAD"
    );
    let hashed_canonical_request = hex::encode(Sha256::digest(canonical_request.as_bytes()));

    let string_to_sign =
        format!("AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{hashed_canonical_request}");

    let signing_key = derive_signing_key(secret_access_key, &date_stamp, region, service);
    let signature = hex::encode(hmac_sha256(&signing_key, string_to_sign.as_bytes()));

    format!("https://{host}{canonical_uri}?{canonical_query_string}&X-Amz-Signature={signature}")
}

fn derive_signing_key(secret: &str, date_stamp: &str, region: &str, service: &str) -> Vec<u8> {
    let k_date = hmac_sha256(format!("AWS4{secret}").as_bytes(), date_stamp.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// Percent-encode per AWS SigV4 rules: unreserved chars pass through, else `%XX`.
fn uri_encode(input: &str, encode_slash: bool) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b'/' if !encode_slash => out.push('/'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Encodes each path segment but preserves `/` separators, matching the object key
/// as it appears in the canonical URI (bucket path is not re-encoded here).
// @spec HIKE-MAP-007
fn uri_path_encode(object_key: &str) -> String {
    object_key
        .split('/')
        .map(|segment| uri_encode(segment, true))
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    // @spec HIKE-REC-005
    #[test]
    fn hike_record_parses_from_expected_r2_json_shape() {
        let json = r#"{
            "id": "blue-ridge",
            "start": "2026-07-18T08:00:00-04:00",
            "end": "2026-07-18T12:00:00-04:00",
            "meeting": { "lat": 37.6, "lon": -79.2 },
            "trails": ["Blue Ridge Loop"],
            "mapKey": "hikes/blue-ridge/map.png"
        }"#;
        let record: HikeRecord = serde_json::from_str(json).unwrap();
        assert_eq!(record.id, "blue-ridge");
        assert_eq!(record.meeting.lat, 37.6);
        assert_eq!(record.map_key, "hikes/blue-ridge/map.png");
        assert_eq!(record.trails, vec!["Blue Ridge Loop".to_string()]);
    }

    /// Records may carry annotations the worker does not read — including the
    /// `start`/`end` the admin still writes for its own UI — and an unknown
    /// key must not fail the whole record.
    // @spec HIKE-REC-004
    #[test]
    fn hike_record_ignores_unknown_fields() {
        let json = r#"{
            "id": "blue-ridge",
            "start": "2026-07-18T08:00:00-04:00",
            "end": "2026-07-18T12:00:00-04:00",
            "meeting": { "lat": 37.6, "lon": -79.2 },
            "trails": ["Blue Ridge Loop"],
            "mapKey": "hikes/blue-ridge/map.png",
            "notes": "bring bug spray",
            "difficulty": 2
        }"#;
        let record: HikeRecord = serde_json::from_str(json).unwrap();
        assert_eq!(record.id, "blue-ridge");
        assert_eq!(record.trails, vec!["Blue Ridge Loop".to_string()]);
    }

    /// A record with no `start`/`end` at all — the shape the admin will write
    /// once it stops sending them — parses the same as one that still carries
    /// them.
    // @spec HIKE-REC-005
    #[test]
    fn parse_hike_record_accepts_a_record_with_no_dates() {
        let json = br#"{
            "id": "blue-ridge",
            "meeting": { "lat": 37.6, "lon": -79.2 },
            "trails": ["Blue Ridge Loop"],
            "mapKey": "hikes/blue-ridge/map.png"
        }"#;
        let record = parse_hike_record(json).unwrap();
        assert_eq!(record.id, "blue-ridge");
    }

    // @spec HIKE-LOC-004
    #[test]
    fn location_list_keeps_stored_order() {
        let json = br#"[
            {"short_name": "oakhurst", "full_name": "Oakhurst"},
            {"short_name": "blackwell", "full_name": "Blackwell"}
        ]"#;
        let list = parse_locations(json).unwrap();
        let slugs: Vec<_> = list.iter().map(|l| l.short_name.as_str()).collect();
        assert_eq!(slugs, ["oakhurst", "blackwell"]);
        assert_eq!(list[1].full_name, "Blackwell");
    }

    // @spec HIKE-LOC-005
    #[test]
    fn empty_location_list_is_valid() {
        assert_eq!(parse_locations(b"[]").unwrap(), vec![]);
    }

    /// Unknown fields are ignored on read and absent on write, so the response
    /// carries only the two fields the wire contract names.
    // @spec HIKE-LOC-006
    #[test]
    fn location_entry_ignores_and_drops_unknown_fields() {
        let json = br#"[{"short_name": "a", "full_name": "A", "note": "x"}]"#;
        let list = parse_locations(json).unwrap();
        assert_eq!(
            serde_json::to_string(&list).unwrap(),
            r#"[{"short_name":"a","full_name":"A"}]"#
        );
    }

    // @spec HIKE-LOC-003
    #[test]
    fn misshapen_location_list_is_an_error() {
        for bad in [
            &b""[..],
            b"not json",
            br#"{"short_name": "a", "full_name": "A"}"#,
            br#"[{"short_name": "a"}]"#,
            br#"[{"short_name": 1, "full_name": "A"}]"#,
            br#"[{"short_name": null, "full_name": "A"}]"#,
        ] {
            assert!(
                parse_locations(bad).is_err(),
                "should reject {:?}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    /// SigV4 encodes each path segment but leaves the separators alone, so a key
    /// with a space signs correctly and still addresses the same object.
    // @spec HIKE-MAP-007
    #[test]
    fn object_key_segments_are_encoded_but_separators_are_not() {
        assert_eq!(
            uri_path_encode("hikes/st james farm/map.png"),
            "hikes/st%20james%20farm/map.png"
        );
        assert_eq!(uri_path_encode("a~b-c_d.e"), "a~b-c_d.e");
    }

    // @spec HIKE-MAP-003, HIKE-MAP-004
    #[test]
    fn presigned_url_has_expected_shape() {
        let now = Utc.with_ymd_and_hms(2026, 7, 18, 9, 0, 0).unwrap();
        let url = presign_get_url(
            now,
            "myaccount",
            "hike-club",
            "hikes/blue-ridge/map.png",
            "AKIDEXAMPLE",
            "secretkey",
            3600,
        );
        assert!(url.starts_with(
            "https://myaccount.r2.cloudflarestorage.com/hike-club/hikes/blue-ridge/map.png?"
        ));
        assert!(url.contains("X-Amz-Algorithm=AWS4-HMAC-SHA256"));
        assert!(url.contains("X-Amz-Expires=3600"));
        assert!(url.contains("X-Amz-Signature="));
    }

    // @spec HIKE-MAP-008
    #[test]
    fn presigned_url_is_deterministic_for_same_inputs() {
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let a = presign_get_url(now, "acct", "b", "k", "id", "secret", 60);
        let b = presign_get_url(now, "acct", "b", "k", "id", "secret", 60);
        assert_eq!(a, b);
    }

    // @spec HIKE-MAP-008
    #[test]
    fn presigned_url_changes_with_object_key() {
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let a = presign_get_url(now, "acct", "b", "one.png", "id", "secret", 60);
        let b = presign_get_url(now, "acct", "b", "two.png", "id", "secret", 60);
        assert_ne!(a, b);
    }
}
