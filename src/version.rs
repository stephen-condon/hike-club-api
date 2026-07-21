//! API versioning via the `x-api-version` request header. Pure logic (parsing +
//! the deprecation registry) lives here so it's unit-tested and covered; the
//! runtime glue that reads the header and stamps response headers is in `lib.rs`.

/// Request header carrying the API version. Parallels `x-api-key` (see `auth`).
pub const API_VERSION_HEADER: &str = "x-api-version";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiVersion {
    V1,
    V2,
}

/// Resolve the requested version from the raw header value.
/// - Missing header → `V1`. ponytail: default-to-v1 for now; to require the
///   header later, change this one arm to `return Err(())`.
/// - `"1"` → V1, `"2"` → V2.
/// - Anything else (unknown version, non-integer) → `Err`, which callers map to 400.
#[allow(clippy::result_unit_err)] // unit error is sufficient; caller maps it to a 400
pub fn parse_version(header: Option<&str>) -> Result<ApiVersion, ()> {
    match header {
        None => Ok(ApiVersion::V1),
        Some("1") => Ok(ApiVersion::V1),
        Some("2") => Ok(ApiVersion::V2),
        Some(_) => Err(()),
    }
}

/// Deprecation registry: the sunset date (RFC 8594 `Sunset` header value, an
/// HTTP-date) for a version, or `None` if it isn't deprecated. Nothing is
/// deprecated yet — to sunset v1, return `Some("<HTTP-date>")` for `V1`.
pub fn sunset(version: ApiVersion) -> Option<&'static str> {
    match version {
        ApiVersion::V1 => None,
        ApiVersion::V2 => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_header_defaults_to_v1() {
        assert_eq!(parse_version(None), Ok(ApiVersion::V1));
    }

    #[test]
    fn known_versions_parse() {
        assert_eq!(parse_version(Some("1")), Ok(ApiVersion::V1));
        assert_eq!(parse_version(Some("2")), Ok(ApiVersion::V2));
    }

    #[test]
    fn unknown_or_nonnumeric_versions_are_rejected() {
        assert_eq!(parse_version(Some("3")), Err(()));
        assert_eq!(parse_version(Some("v2")), Err(()));
        assert_eq!(parse_version(Some("")), Err(()));
    }

    #[test]
    fn no_version_is_deprecated_yet() {
        assert!(sunset(ApiVersion::V1).is_none());
        assert!(sunset(ApiVersion::V2).is_none());
    }
}
