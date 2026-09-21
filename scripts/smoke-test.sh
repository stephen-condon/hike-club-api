#!/usr/bin/env bash
# Post-deploy smoke test against a deployed worker (prod or preview).
#
# This is where the Workers-runtime glue is verified. `src/lib.rs`,
# `src/r2_adapter.rs` and `src/weather_adapter.rs` cannot execute under
# `cargo test`, so they are excluded from the coverage gate and covered here
# instead: routing, API-key enforcement, version negotiation and the status
# codes they produce. Assertions carry the EARS ids they exercise.
#
# Usage: scripts/smoke-test.sh <base-url> <api-key>

set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "Usage: $0 <base-url> <api-key>" >&2
  exit 1
fi

base_url="$1"
api_key="$2"
body_file="$(mktemp)"
head_file="$(mktemp)"
trap 'rm -f "$body_file" "$head_file"' EXIT
failures=0

# Issues a GET and asserts the status code. Extra args are passed to curl, so
# callers add headers. Response body and headers are left in $body_file and
# $head_file for further assertions.
check_status() {
  local label="$1" expected="$2" path="$3"; shift 3
  local actual
  actual=$(curl -s -o "$body_file" -D "$head_file" -w '%{http_code}' "$@" "${base_url}${path}")
  if [ "$actual" = "$expected" ]; then
    echo "  ok   $label ($expected)"
  else
    echo "  FAIL $label: expected $expected, got $actual" >&2
    sed 's/^/       /' "$body_file" >&2
    failures=$((failures + 1))
  fi
}

# Asserts the response body matches a pattern, against the last check_status.
check_body() {
  local label="$1" pattern="$2"
  if grep -qE "$pattern" "$body_file"; then
    echo "  ok   $label"
  else
    echo "  FAIL $label: body does not match '$pattern'" >&2
    sed 's/^/       /' "$body_file" >&2
    failures=$((failures + 1))
  fi
}

# Asserts a response header matches a pattern, against the last check_status.
check_header() {
  local label="$1" pattern="$2"
  if grep -qi "$pattern" "$head_file"; then
    echo "  ok   $label"
  else
    echo "  FAIL $label: no header matching '$pattern'" >&2
    failures=$((failures + 1))
  fi
}

echo "smoke test: $base_url"

# @spec API-ROUTE-001 — health is open, no key and no version header.
check_status "health is served unauthenticated" 200 "/health"

# @spec API-ROUTE-002 — an unrouted path is 501; 404 is reserved for a missing hike.
check_status "unrouted path" 501 "/not-a-route"
check_status "root path" 501 "/"

# @spec API-ROUTE-003 — a routed path under a method it does not serve is 405
# with an Allow header naming what it does serve, and is answered without a key:
# the path exists, the method does not.
check_status "health under POST" 405 "/health" -X POST
check_header "405 names the supported methods" '^allow:.*GET'
check_status "hike under DELETE" 405 "/hike/smoke-test" -X DELETE
check_status "locations under PUT" 405 "/hike-locations" -X PUT

# @spec API-AUTH-003 — a missing key is rejected before anything else happens.
check_status "hike without an api key" 401 "/hike/smoke-test" \
  -H "x-api-version: 2"

# @spec API-AUTH-002, API-AUTH-003 — the locations endpoint is behind the same key.
check_status "locations without an api key" 401 "/hike-locations" \
  -H "x-api-version: 2"

# @spec API-WIRE-006 — error bodies are plain text, not JSON.
if head -c 1 "$body_file" | grep -q '{'; then
  echo "  FAIL error body is JSON, expected plain text" >&2
  failures=$((failures + 1))
else
  echo "  ok   error body is plain text"
fi

# @spec API-AUTH-004 — admission order: a request that is wrong in both ways is
# rejected for the key, because authorization runs before version negotiation.
check_status "no key and a bad version is a 401, not a 400" 401 "/hike/smoke-test" \
  -H "x-api-version: 99"

# @spec API-VER-007 — an error response carries no deprecation headers; a client
# debugging a 401 is not being told about a sunset.
if grep -qiE '^(deprecation|sunset):' "$head_file"; then
  echo "  FAIL error response carries deprecation headers" >&2
  failures=$((failures + 1))
else
  echo "  ok   error response carries no deprecation headers"
fi

# @spec API-VER-001 — the version header is required, never defaulted.
check_status "hike without a version header" 400 "/hike/smoke-test" \
  -H "x-api-key: $api_key"

# @spec API-VER-002 — an unknown version is rejected, not rounded to a known one.
check_status "hike with an unknown version" 400 "/hike/smoke-test" \
  -H "x-api-key: $api_key" -H "x-api-version: 99"

# @spec API-VER-002 — a non-integer version is rejected the same way.
check_status "hike with a non-integer version" 400 "/hike/smoke-test" \
  -H "x-api-key: $api_key" -H "x-api-version: v2"

# @spec API-VER-005 — a version past its sunset is gone, not merely deprecated,
# on every endpoint that negotiates a version. The body names the version, the
# date it went, and what to ask for instead.
check_status "hike under a sunset version" 410 "/hike/smoke-test" \
  -H "x-api-key: $api_key" -H "x-api-version: 1"
check_body "410 names the version, its sunset and the live versions" \
  '^api version 1 was sunset on .+; supported versions: .+$'
check_status "locations under a sunset version" 410 "/hike-locations" \
  -H "x-api-key: $api_key" -H "x-api-version: 1"

# @spec API-VER-007 — the 410 carries no Sunset header; the sunset is the body.
if grep -qiE '^(deprecation|sunset):' "$head_file"; then
  echo "  FAIL the 410 carries deprecation headers" >&2
  failures=$((failures + 1))
else
  echo "  ok   the 410 carries no deprecation headers"
fi

# @spec API-RESP-004, HIKE-REC-002 — a hike that does not exist is a 404, not an error.
check_status "hike that does not exist" 404 "/hike/definitely-not-a-hike" \
  -H "x-api-key: $api_key" -H "x-api-version: 2"

# @spec API-ROUTE-004 — /hike and /hike/ address the collection and name nothing
# in it: a hike that does not exist, not an unrouted path and not a bad request.
check_status "hike collection with no id" 404 "/hike" \
  -H "x-api-key: $api_key" -H "x-api-version: 2"
check_status "hike collection with a trailing slash" 404 "/hike/" \
  -H "x-api-key: $api_key" -H "x-api-version: 2"

# @spec API-LOC-001, API-LOC-002 — the location mapping is served as JSON.
check_status "locations are served" 200 "/hike-locations" \
  -H "x-api-key: $api_key" -H "x-api-version: 2"
check_header "locations carry a json content-type" '^content-type:.*application/json'

# @spec API-VER-003 — the locations payload does not vary by version. Compared
# across the versions still served, which is what "every supported version"
# means: a sunset version answers 410 and has no payload to compare. Add each
# new version here as it is registered.
live_versions=(2 3)
for v in "${live_versions[@]}"; do
  curl -s -H "x-api-key: $api_key" -H "x-api-version: $v" \
    "${base_url}/hike-locations" > "$body_file.v$v"
done
if ! ls "$body_file".v* >/dev/null 2>&1; then
  echo "  FAIL no live versions to compare locations across" >&2
  failures=$((failures + 1))
else
  first="$body_file.v${live_versions[0]}"
  identical=1
  for v in "${live_versions[@]}"; do
    cmp -s "$first" "$body_file.v$v" || identical=0
  done
  if [ "$identical" = 1 ]; then
    echo "  ok   locations are identical across the served versions"
  else
    echo "  FAIL locations differ between served versions" >&2
    failures=$((failures + 1))
  fi
fi
rm -f "$body_file".v*

# @spec API-RESP-001, API-WIRE-003 — the fixture hike round-trips end to end.
check_status "fixture hike is served" 200 "/hike/smoke-test" \
  -H "x-api-key: $api_key" -H "x-api-version: 2"

# @spec API-WIRE-005, HIKE-MAP-001 — the map is a presigned URL with an expiry.
if grep -q '"url":"https://[^"]*X-Amz-Signature=' "$body_file" \
   && grep -qE '"expiresAt":"[0-9]{4}-[0-9]{2}-[0-9]{2}T' "$body_file"; then
  echo "  ok   map is a presigned url with an rfc 3339 expiry"
else
  echo "  FAIL map is not a presigned url with an expiry" >&2
  sed 's/^/       /' "$body_file" >&2
  failures=$((failures + 1))
fi

# @spec API-VER-006, API-VER-008 — v2 is deprecated: served in full, with its
# sunset advertised in RFC 8594 headers.
check_status "hike under deprecated v2" 200 "/hike/smoke-test" \
  -H "x-api-key: $api_key" -H "x-api-version: 2"
check_header "v2 carries Deprecation" '^deprecation: true'
check_header "v2 carries its Sunset date" '^sunset: Wed, 18 Nov 2026 00:00:00 GMT'
check_header "v2 links the deprecation doc" '^link:.*rel="deprecation"'

# @spec API-VER-008 — v3 is current: served, with no deprecation headers.
check_status "hike under current v3" 200 "/hike/smoke-test" \
  -H "x-api-key: $api_key" -H "x-api-version: 3"
if grep -qiE '^(deprecation|sunset):' "$head_file"; then
  echo "  FAIL v3 carries deprecation headers" >&2
  failures=$((failures + 1))
else
  echo "  ok   v3 carries no deprecation headers"
fi

if [ "$failures" -ne 0 ]; then
  echo "smoke test failed: $failures assertion(s)" >&2
  exit 1
fi

echo "smoke test passed"
