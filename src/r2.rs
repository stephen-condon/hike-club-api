use crate::models::HikeRecord;
use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// Abstraction over hike storage so handlers can be unit-tested without R2/network.
/// Real impl (`R2HikeStore`) reads the R2 binding; tests use an in-memory fake.
pub trait HikeStore {
    async fn get_hike(&self, id: &str) -> Result<Option<HikeRecord>, String>;
    async fn presign_map_url(&self, map_key: &str) -> Result<(String, DateTime<Utc>), String>;
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

    #[test]
    fn hike_record_parses_from_expected_r2_json_shape() {
        let json = r#"{
            "id": "2026-07-18-blue-ridge",
            "start": "2026-07-18T08:00:00-04:00",
            "end": "2026-07-18T12:00:00-04:00",
            "meeting": { "lat": 37.6, "lon": -79.2 },
            "trails": ["Blue Ridge Loop"],
            "mapKey": "hikes/2026-07-18-blue-ridge/map.png"
        }"#;
        let record: HikeRecord = serde_json::from_str(json).unwrap();
        assert_eq!(record.id, "2026-07-18-blue-ridge");
        assert_eq!(record.meeting.lat, 37.6);
        assert_eq!(record.map_key, "hikes/2026-07-18-blue-ridge/map.png");
        assert_eq!(record.trails, vec!["Blue Ridge Loop".to_string()]);
    }

    #[test]
    fn presigned_url_has_expected_shape() {
        let now = Utc.with_ymd_and_hms(2026, 7, 18, 9, 0, 0).unwrap();
        let url = presign_get_url(
            now,
            "myaccount",
            "hike-club",
            "hikes/2026-07-18-blue-ridge/map.png",
            "AKIDEXAMPLE",
            "secretkey",
            3600,
        );
        assert!(url.starts_with("https://myaccount.r2.cloudflarestorage.com/hike-club/hikes/2026-07-18-blue-ridge/map.png?"));
        assert!(url.contains("X-Amz-Algorithm=AWS4-HMAC-SHA256"));
        assert!(url.contains("X-Amz-Expires=3600"));
        assert!(url.contains("X-Amz-Signature="));
    }

    #[test]
    fn presigned_url_is_deterministic_for_same_inputs() {
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let a = presign_get_url(now, "acct", "b", "k", "id", "secret", 60);
        let b = presign_get_url(now, "acct", "b", "k", "id", "secret", 60);
        assert_eq!(a, b);
    }

    #[test]
    fn presigned_url_changes_with_object_key() {
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let a = presign_get_url(now, "acct", "b", "one.png", "id", "secret", 60);
        let b = presign_get_url(now, "acct", "b", "two.png", "id", "secret", 60);
        assert_ne!(a, b);
    }
}
