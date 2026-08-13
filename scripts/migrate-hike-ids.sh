#!/usr/bin/env bash
# One-shot migration: hikes/YYYY-MM-DD-<slug>.json  ->  hikes/<slug>.json
#
# Hike ids dropped their date component (see scripts/upload-hike.sh). This moves
# the existing dated metadata objects onto the new keys and deletes the originals.
# Safe to delete this script once it has been run against the bucket.
#
# Usage:
#   AWS_ACCESS_KEY_ID=<r2 key id> AWS_SECRET_ACCESS_KEY=<r2 secret> \
#     scripts/migrate-hike-ids.sh [--apply]
#
# Credentials are the same R2 API token as the R2_ACCESS_KEY_ID /
# R2_SECRET_ACCESS_KEY worker secrets. wrangler has no object-list command, so
# this talks to R2 over its S3-compatible endpoint via the aws CLI.
#
# Default is a dry run: it prints exactly what it would back up, write, and
# delete, and touches nothing. Pass --apply to execute.

set -euo pipefail

ACCOUNT_ID="d1c32a549c7dddaa7c03b13b1d8df178"
BUCKET="hike-club-api"
ENDPOINT="https://${ACCOUNT_ID}.r2.cloudflarestorage.com"
export AWS_DEFAULT_REGION="auto"

apply=false
case "${1:-}" in
  --apply) apply=true ;;
  --dry-run | "") ;;
  *) echo "Usage: $0 [--apply]" >&2; exit 1 ;;
esac

for cmd in aws jq; do
  command -v "$cmd" >/dev/null || { echo "error: $cmd not found" >&2; exit 1; }
done
: "${AWS_ACCESS_KEY_ID:?set AWS_ACCESS_KEY_ID to the R2 access key id}"
: "${AWS_SECRET_ACCESS_KEY:?set AWS_SECRET_ACCESS_KEY to the R2 secret access key}"

s3() { aws s3api --endpoint-url "$ENDPOINT" "$@"; }

backup_dir="r2-backup-$(date +%Y%m%d-%H%M%S)"
work_dir="$(mktemp -d)"
trap 'rm -rf "$work_dir"' EXIT

$apply || echo "DRY RUN — nothing will be written or deleted. Re-run with --apply to execute."
echo

# --- 1. list dated metadata keys -------------------------------------------
all_keys="$(s3 list-objects-v2 --bucket "$BUCKET" --prefix "hikes/" \
  --query 'Contents[].Key' --output text | tr '\t' '\n')"

# hikes/YYYY-MM-DD-<slug>.json — undated records (e.g. hikes/smoke-test.json)
# don't match and are left alone.
dated_keys="$(printf '%s\n' "$all_keys" \
  | grep -E '^hikes/[0-9]{4}-[0-9]{2}-[0-9]{2}-[^/]+\.json$' || true)"

if [ -z "$dated_keys" ]; then
  echo "No dated hikes/*.json objects found — nothing to migrate."
  exit 0
fi

echo "Found $(printf '%s\n' "$dated_keys" | wc -l | tr -d ' ') dated metadata object(s):"
printf '  %s\n' $dated_keys
echo

# --- 2. back up locally BEFORE any write or delete --------------------------
# The delete below is against the only copy of this data.
echo "Backup -> ${backup_dir}/"
$apply && mkdir -p "$backup_dir"
while IFS= read -r key; do
  echo "  ${key}"
  $apply && s3 get-object --bucket "$BUCKET" --key "$key" \
    "${backup_dir}/$(basename "$key")" >/dev/null
done <<< "$dated_keys"
echo

# --- 3. fetch bodies, resolve slug collisions (latest "start" wins) ---------
# winners: slug -> key, indexed by file so this works on bash 3.2 (macOS).
while IFS= read -r key; do
  base="$(basename "$key" .json)"
  slug="${base:11}"  # strip "YYYY-MM-DD-"
  body="${work_dir}/body-${base}.json"
  s3 get-object --bucket "$BUCKET" --key "$key" "$body" >/dev/null
  start="$(jq -r '.start // ""' "$body")"
  printf '%s\t%s\t%s\t%s\n' "$slug" "$start" "$key" "$body"
done <<< "$dated_keys" | sort -t"$(printf '\t')" -k1,1 -k2,2r > "${work_dir}/records"

collisions=false
: > "${work_dir}/winners"
prev_slug=""
while IFS="$(printf '\t')" read -r slug start key body; do
  if [ "$slug" = "$prev_slug" ]; then
    echo "  COLLISION: dropping ${key} (start ${start}) — a later record for '${slug}' wins"
    collisions=true
    continue
  fi
  prev_slug="$slug"
  printf '%s\t%s\t%s\t%s\n' "$slug" "$start" "$key" "$body" >> "${work_dir}/winners"
done < "${work_dir}/records"
$collisions && echo

# --- 4/5. rewrite the id field to match the new key, then put ---------------
# The API echoes "id" in the response, so key and body must agree.
echo "Writes:"
while IFS="$(printf '\t')" read -r slug start key body; do
  map_key="$(jq -r '.mapKey // ""' "$body")"
  if [ "$map_key" != "hikes/${slug}/map.png" ]; then
    # Left as-is on purpose: the map object lives where it lives, and nothing
    # here deletes maps. Reported so it can be reconciled by hand.
    echo "  note: ${slug} has mapKey '${map_key}' (expected hikes/${slug}/map.png) — leaving it alone"
  fi
  jq --arg slug "$slug" '.id = $slug' "$body" > "${body}.new"
  echo "  ${key} -> hikes/${slug}.json  (id -> \"${slug}\", start ${start})"
  $apply && s3 put-object --bucket "$BUCKET" --key "hikes/${slug}.json" \
    --body "${body}.new" --content-type application/json >/dev/null
done < "${work_dir}/winners"
echo

# --- 6. delete the dated keys, only after their put succeeded ---------------
echo "Deletes:"
while IFS= read -r key; do
  echo "  ${key}"
  $apply && s3 delete-object --bucket "$BUCKET" --key "$key" >/dev/null
done <<< "$dated_keys"
echo

# --- 7. report dated map prefixes as separate cleanup ----------------------
dated_maps="$(printf '%s\n' "$all_keys" \
  | grep -E '^hikes/[0-9]{4}-[0-9]{2}-[0-9]{2}-[^/]+/' || true)"
if [ -n "$dated_maps" ]; then
  echo "Leftover objects under dated prefixes (NOT touched — clean up separately):"
  printf '  %s\n' $dated_maps
  echo
fi

if $apply; then
  echo "Done. Backups in ${backup_dir}/"
else
  echo "Dry run complete. Re-run with --apply to execute."
fi
