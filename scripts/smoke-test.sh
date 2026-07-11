#!/usr/bin/env bash
# Post-deploy smoke test: confirms a deployed worker (prod or preview) is
# actually reachable and serving the fixed `smoke-test` hike fixture.
#
# Usage: scripts/smoke-test.sh <base-url> <api-key>

set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "Usage: $0 <base-url> <api-key>" >&2
  exit 1
fi

base_url="$1"
api_key="$2"
response_file="$(mktemp)"
trap 'rm -f "$response_file"' EXIT

health_status=$(curl -s -o /dev/null -w '%{http_code}' "$base_url/health")
if [ "$health_status" != "200" ]; then
  echo "health check failed: $health_status" >&2
  exit 1
fi

hike_status=$(curl -s -o "$response_file" -w '%{http_code}' \
  -H "x-api-key: $api_key" \
  "$base_url/hike/smoke-test")
if [ "$hike_status" != "200" ]; then
  echo "hike smoke test failed: $hike_status" >&2
  cat "$response_file" >&2
  exit 1
fi

echo "smoke test passed"
