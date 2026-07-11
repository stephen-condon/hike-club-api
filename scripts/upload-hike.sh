#!/usr/bin/env bash
# Uploads one hike's metadata JSON + trail map image to R2.
#
# Usage: scripts/upload-hike.sh <hike-id> <metadata.json> <map.png>
#
# <metadata.json> must match HikeRecord in src/models.rs, e.g.:
#   {
#     "id": "2026-07-18-blue-ridge",
#     "start": "2026-07-18T08:00:00-04:00",
#     "end": "2026-07-18T12:00:00-04:00",
#     "meeting": { "lat": 37.6, "lon": -79.2 },
#     "trails": ["Blue Ridge Loop"],
#     "mapKey": "hikes/2026-07-18-blue-ridge/map.png"
#   }
# "mapKey" must equal "hikes/<hike-id>/map.png" so it matches where this
# script uploads the image.

set -euo pipefail

BUCKET="hike-club-api"

if [ "$#" -ne 3 ]; then
  echo "Usage: $0 <hike-id> <metadata.json> <map.png>" >&2
  exit 1
fi

id="$1"
metadata="$2"
map="$3"

for f in "$metadata" "$map"; do
  if [ ! -f "$f" ]; then
    echo "error: file not found: $f" >&2
    exit 1
  fi
done

wrangler r2 object put "${BUCKET}/hikes/${id}.json" --file "$metadata" --content-type application/json --remote
wrangler r2 object put "${BUCKET}/hikes/${id}/map.png" --file "$map" --content-type image/png --remote

echo "Uploaded hike '${id}' to r2://${BUCKET}/hikes/${id}.json and .../map.png"
