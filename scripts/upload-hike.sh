#!/usr/bin/env bash
# Uploads one hike's metadata JSON + trail map image to R2.
#
# Usage: scripts/upload-hike.sh <hike-id> <metadata.json> <map.png>
#
# <hike-id> is the location slug — the "short_name" in
# resources/hike-location-mapping.json, matching a file stem in location_based/.
# No date component: one record per location, overwritten each time that location
# is scheduled. That keeps /hike/{id} links permanent across reschedules.
#
# <metadata.json> must match HikeRecord in src/models.rs, e.g.:
#   {
#     "id": "blue-ridge",
#     "start": "2026-07-18T08:00:00-04:00",
#     "end": "2026-07-18T12:00:00-04:00",
#     "meeting": { "lat": 37.6, "lon": -79.2 },
#     "trails": ["Blue Ridge Loop"],
#     "mapKey": "hikes/blue-ridge/map.png"
#   }
# "mapKey" must equal "hikes/<hike-id>/map.png" so it matches where this
# script uploads the image.
#
# Since the id no longer carries the date, "start"/"end" are the ONLY record of
# when the hike happens — set them to the upcoming date before every upload. A
# record left with a past "end" makes the API serve observed weather for the last
# hike as though it were the current one.

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
