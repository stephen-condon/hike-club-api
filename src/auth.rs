/// Header the iOS client must send; the WAF rule at the edge checks the same
/// header, this is the defense-in-depth check inside the worker itself.
pub const API_KEY_HEADER: &str = "x-api-key";

/// Constant-time-ish compare is unnecessary here: this isn't a crypto secret
/// comparison against a signature, it's a single shared key behind a WAF rule
/// already filtering unauthenticated traffic before it reaches the worker.
pub fn is_authorized(provided: Option<&str>, expected: &str) -> bool {
    matches!(provided, Some(key) if key == expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_matching_key() {
        assert!(is_authorized(Some("secret"), "secret"));
    }

    #[test]
    fn rejects_missing_key() {
        assert!(!is_authorized(None, "secret"));
    }

    #[test]
    fn rejects_wrong_key() {
        assert!(!is_authorized(Some("nope"), "secret"));
    }
}
