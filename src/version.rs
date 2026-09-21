//! API versioning via the `x-api-version` request header. Pure logic (parsing +
//! the version registry) lives here so it's unit-tested and covered; the
//! runtime glue that reads the header and stamps response headers is in `lib.rs`.

use chrono::{DateTime, Utc};

/// Request header carrying the API version. Parallels `x-api-key` (see `auth`).
pub const API_VERSION_HEADER: &str = "x-api-version";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiVersion {
    V1,
    V2,
}

/// The version registry: every version the system knows, the header value that
/// names it, and its sunset date as an RFC 8594 `Sunset` header value (an
/// HTTP-date), or `None` while the version is current.
///
/// A version stays here after its response shape is removed — the entry is what
/// makes a request naming it a 410 rather than the 400 given to a version that
/// never existed. Whether a dated version is merely deprecated or already gone
/// is not recorded but derived from the current time; see `status`.
// @spec API-VER-004
const REGISTRY: &[(ApiVersion, &str, Option<&str>)] = &[
    (ApiVersion::V1, "1", Some("Thu, 20 Aug 2026 00:00:00 GMT")),
    (ApiVersion::V2, "2", None),
];

/// Where a version stands at a given instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Current,
    Deprecated { sunset: &'static str },
    Sunset { sunset: &'static str },
}

/// Resolve the requested version from the raw header value. A missing header,
/// an unknown version and a non-integer are all `Err`, which callers map to 400.
#[allow(clippy::result_unit_err)] // unit error is sufficient; caller maps it to a 400
// @spec API-VER-001, API-VER-002
pub fn parse_version(header: Option<&str>) -> Result<ApiVersion, ()> {
    let header = header.ok_or(())?;
    REGISTRY
        .iter()
        .find(|(_, label, _)| *label == header)
        .map(|(version, _, _)| *version)
        .ok_or(())
}

/// The registry's sunset date for a version, or `None` if it has none.
// @spec API-VER-004
pub fn sunset(version: ApiVersion) -> Option<&'static str> {
    REGISTRY
        .iter()
        .find(|(v, _, _)| *v == version)
        .and_then(|(_, _, sunset)| *sunset)
}

/// A version's status against a supplied `now`, so sunset enforcement is
/// exercised in tests without the Workers runtime clock. A date at or before
/// `now` is past; an unparseable date is treated as no date at all, which
/// `every_registry_sunset_date_parses` is there to prevent.
// @spec API-VER-004, API-VER-010
pub fn status(version: ApiVersion, now: DateTime<Utc>) -> Status {
    match sunset(version) {
        None => Status::Current,
        Some(date) => match DateTime::parse_from_rfc2822(date) {
            Ok(d) if d.to_utc() <= now => Status::Sunset { sunset: date },
            Ok(_) => Status::Deprecated { sunset: date },
            Err(_) => Status::Current,
        },
    }
}

/// The header values of every version not past its sunset, in registry order.
// @spec API-VER-005
pub fn live_versions(now: DateTime<Utc>) -> Vec<&'static str> {
    REGISTRY
        .iter()
        .filter(|(version, _, _)| !matches!(status(*version, now), Status::Sunset { .. }))
        .map(|(_, label, _)| *label)
        .collect()
}

/// The 410 body for a version past its sunset, or `None` while it is still
/// served. It names the version, its date, and what to ask for instead, so a
/// stale client learns both that it is finished and where to go.
// @spec API-VER-005
pub fn gone_body(version: ApiVersion, now: DateTime<Utc>) -> Option<String> {
    let Status::Sunset { sunset } = status(version, now) else {
        return None;
    };
    let label = REGISTRY
        .iter()
        .find(|(v, _, _)| *v == version)
        .map(|(_, label, _)| *label)
        .unwrap_or_default();
    Some(format!(
        "api version {label} was sunset on {sunset}; supported versions: {}",
        live_versions(now).join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// Midnight UTC on the given day, the granularity every sunset date uses.
    fn at(y: i32, m: u32, d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, 0, 0, 0).unwrap()
    }

    // @spec API-VER-001
    #[test]
    fn missing_header_is_rejected() {
        assert_eq!(parse_version(None), Err(()));
    }

    // @spec API-VER-001
    #[test]
    fn known_versions_parse() {
        assert_eq!(parse_version(Some("1")), Ok(ApiVersion::V1));
        assert_eq!(parse_version(Some("2")), Ok(ApiVersion::V2));
    }

    // @spec API-VER-002
    #[test]
    fn unknown_or_nonnumeric_versions_are_rejected() {
        assert_eq!(parse_version(Some("3")), Err(()));
        assert_eq!(parse_version(Some("v2")), Err(()));
        assert_eq!(parse_version(Some("")), Err(()));
    }

    // @spec API-VER-006
    #[test]
    fn v1_is_deprecated() {
        assert_eq!(
            sunset(ApiVersion::V1),
            Some("Thu, 20 Aug 2026 00:00:00 GMT")
        );
        assert!(sunset(ApiVersion::V2).is_none());
    }

    /// A typo in a registry date must fail here rather than silently reading as
    /// "not sunset" at runtime.
    // @spec API-VER-004
    #[test]
    fn every_registry_sunset_date_parses() {
        for (_, label, sunset) in REGISTRY {
            if let Some(date) = sunset {
                assert!(
                    DateTime::parse_from_rfc2822(date).is_ok(),
                    "version {label} has an unparseable sunset date: {date}"
                );
            }
        }
    }

    // @spec API-VER-004
    #[test]
    fn a_version_with_no_sunset_is_current() {
        assert_eq!(status(ApiVersion::V2, at(2026, 9, 20)), Status::Current);
    }

    // @spec API-VER-004, API-VER-006
    #[test]
    fn a_version_before_its_sunset_is_deprecated() {
        assert_eq!(
            status(ApiVersion::V1, at(2026, 8, 19)),
            Status::Deprecated {
                sunset: "Thu, 20 Aug 2026 00:00:00 GMT"
            }
        );
    }

    /// "at or before the current time": the sunset instant itself is already gone.
    // @spec API-VER-010
    #[test]
    fn a_version_at_or_past_its_sunset_is_sunset() {
        let gone = Status::Sunset {
            sunset: "Thu, 20 Aug 2026 00:00:00 GMT",
        };
        assert_eq!(status(ApiVersion::V1, at(2026, 8, 20)), gone);
        assert_eq!(status(ApiVersion::V1, at(2026, 8, 21)), gone);
    }

    // @spec API-VER-005
    #[test]
    fn live_versions_exclude_the_sunset_ones() {
        assert_eq!(live_versions(at(2026, 8, 19)), vec!["1", "2"]);
        assert_eq!(live_versions(at(2026, 8, 20)), vec!["2"]);
    }

    // @spec API-VER-005
    #[test]
    fn a_sunset_version_gets_a_body_naming_the_date_and_the_live_versions() {
        assert_eq!(
            gone_body(ApiVersion::V1, at(2026, 9, 20)).as_deref(),
            Some(
                "api version 1 was sunset on Thu, 20 Aug 2026 00:00:00 GMT; supported versions: 2"
            )
        );
    }

    // @spec API-VER-005
    #[test]
    fn a_version_that_is_not_sunset_gets_no_body() {
        assert_eq!(gone_body(ApiVersion::V1, at(2026, 8, 19)), None);
        assert_eq!(gone_body(ApiVersion::V2, at(2026, 9, 20)), None);
    }
}
