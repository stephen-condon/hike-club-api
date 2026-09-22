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

# @spec API-ROUTE-005 — routing is settled before the key is evaluated, so a
# wrong key does not turn an unrouted path or an unsupported method into a 401.
check_status "unrouted path with a wrong key" 501 "/not-a-route" \
  -H "x-api-key: not-the-key"
check_status "unsupported method with a wrong key" 405 "/hike/smoke-test" -X DELETE \
  -H "x-api-key: not-the-key"

# @spec API-AUTH-003 — a missing key is rejected before anything else happens.
check_status "hike without an api key" 401 "/hike/smoke-test" \
  -H "x-api-version: 3"

# @spec API-AUTH-002, API-AUTH-003, API-LOC-004 — the locations endpoint is behind
# the same key, which is checked before the location list is read.
check_status "locations without an api key" 401 "/hike-locations" \
  -H "x-api-version: 3"

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

# @spec API-VER-005, API-VER-009 — a version past its sunset is gone, not merely
# deprecated, on every endpoint that negotiates a version: it stays registered, so
# it answers 410 rather than the 400 for an unknown version. The body names the
# version, the date it went, and what to ask for instead. v1 and v2 both check
# this, since both are now past their sunset.
check_status "hike under a v1-sunset version" 410 "/hike/smoke-test" \
  -H "x-api-key: $api_key" -H "x-api-version: 1"
check_body "410 names the version, its sunset and the live versions" \
  '^api version 1 was sunset on .+; supported versions: .+$'
check_status "locations under a v1-sunset version" 410 "/hike-locations" \
  -H "x-api-key: $api_key" -H "x-api-version: 1"
check_status "hike under a v2-sunset version" 410 "/hike/smoke-test" \
  -H "x-api-key: $api_key" -H "x-api-version: 2"
check_body "410 names v2, its sunset and the live versions" \
  '^api version 2 was sunset on .+; supported versions: .+$'
check_status "locations under a v2-sunset version" 410 "/hike-locations" \
  -H "x-api-key: $api_key" -H "x-api-version: 2"

# @spec API-VER-007 — the 410 carries no Sunset header; the sunset is the body.
if grep -qiE '^(deprecation|sunset):' "$head_file"; then
  echo "  FAIL the 410 carries deprecation headers" >&2
  failures=$((failures + 1))
else
  echo "  ok   the 410 carries no deprecation headers"
fi

# @spec API-RESP-004, HIKE-REC-002 — a hike that does not exist is a 404, not an error.
check_status "hike that does not exist" 404 "/hike/definitely-not-a-hike" \
  -H "x-api-key: $api_key" -H "x-api-version: 3"

# @spec API-ROUTE-004 — /hike and /hike/ address the collection and name nothing
# in it: a hike that does not exist, not an unrouted path and not a bad request.
check_status "hike collection with no id" 404 "/hike" \
  -H "x-api-key: $api_key" -H "x-api-version: 3"
check_status "hike collection with a trailing slash" 404 "/hike/" \
  -H "x-api-key: $api_key" -H "x-api-version: 3"

# @spec API-LOC-001, API-LOC-002, HIKE-LOC-001, HIKE-CFG-002 — the location list is
# read from R2 through the HIKES binding and served as JSON.
check_status "locations are served" 200 "/hike-locations" \
  -H "x-api-key: $api_key" -H "x-api-version: 3"
check_header "locations carry a json content-type" '^content-type:.*application/json'

# @spec API-VER-003 — the locations payload does not vary by version. Compared
# across the versions still served, which is what "every supported version"
# means: a sunset version answers 410 and has no payload to compare. Add each
# new version here as it is registered.
live_versions=(3)
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

# @spec API-VER-008, API-WIN-001 — v3 is current: served, with no deprecation
# headers, taking the hike's window as query params.
check_status "fixture hike is served under current v3" 200 \
  "/hike/smoke-test?start=2026-01-01T08%3A00%3A00-05%3A00&end=2026-01-01T12%3A00%3A00-05%3A00" \
  -H "x-api-key: $api_key" -H "x-api-version: 3"
if grep -qiE '^(deprecation|sunset):' "$head_file"; then
  echo "  FAIL v3 carries deprecation headers" >&2
  failures=$((failures + 1))
else
  echo "  ok   v3 carries no deprecation headers"
fi

# @spec API-WIRE-005, HIKE-MAP-001 — the map is a presigned URL with an expiry.
if grep -q '"url":"https://[^"]*X-Amz-Signature=' "$body_file" \
   && grep -qE '"expiresAt":"[0-9]{4}-[0-9]{2}-[0-9]{2}T' "$body_file"; then
  echo "  ok   map is a presigned url with an rfc 3339 expiry"
else
  echo "  FAIL map is not a presigned url with an expiry" >&2
  sed 's/^/       /' "$body_file" >&2
  failures=$((failures + 1))
fi

# @spec API-WIRE-009 — v3 answers in its own shape: a map flagged available, and
# no bare `conditions` field.
check_body "v3 flags its map as available" '"mapAvailable":true'
if grep -q '"conditions":' "$body_file"; then
  echo "  FAIL v3 carries a bare conditions field" >&2
  failures=$((failures + 1))
else
  echo "  ok   v3 carries no bare conditions field"
fi

# @spec API-WIRE-011 — v3 carries no start/end: the client already knows the
# window it asked for.
if grep -qE '"(start|end)":' "$body_file"; then
  echo "  FAIL v3 carries start/end" >&2
  failures=$((failures + 1))
else
  echo "  ok   v3 carries no start/end"
fi

# @spec API-WIN-002 — v3 requires the window query params, 400 before storage.
check_status "v3 without a window" 400 "/hike/smoke-test" \
  -H "x-api-key: $api_key" -H "x-api-version: 3"
check_body "v3 without a window names the missing parameter" 'start query parameter is required'

if [ "$failures" -ne 0 ]; then
  echo "smoke test failed: $failures assertion(s)" >&2
  exit 1
fi

echo "smoke test passed"
