---
parent: high-level-design
prefix: HIKE
---

# Hike Record

## Context and Design Philosophy

Hike content — when, where, which trails, and the trail map — lives in R2 and nowhere else. This segment owns the bucket's object layout, the record schema, retrieval, and the presigned URLs that let a client fetch a map image without the worker touching its bytes.

Two constraints shape everything here. The worker must not proxy megabyte-scale images, because CPU time and bandwidth are the free tier's scarcest resources. And there is no authoring application: records are written by an organizer running a script, which makes the schema's readability and the layout's predictability part of the design rather than an implementation detail.

## Object Layout

```
hike-club-api/                        (R2 bucket)
├── hikes/{id}.json                   the hike record
└── hikes/{id}/map.png                the trail map
```

`{id}` is a location slug — the `short_name` from `resources/hike-location-mapping.json`, such as `blackwell-forest-preserve`. It carries no date component.

One object per location, overwritten each time that location is scheduled. This makes `/hike/{id}` links permanent across reschedules and keeps the bucket's object count equal to the number of preserves the pack visits rather than growing without bound. The cost is that a hike's `start`/`end` are the only statement of when it happens, and a record left with last season's dates is served as though current — the failure mode the upload path has to guard against.

## Record Schema

| Field | Type | Notes |
|---|---|---|
| `id` | string | The location slug. Echoed into the response. |
| `start` | string | RFC 3339 with offset, e.g. `2026-09-20T08:00:00-05:00`. |
| `end` | string | RFC 3339 with offset. |
| `meeting.lat` / `meeting.lon` | number | Trailhead coordinates; also the weather query point. |
| `trails` | string array | Trail names as the pack refers to them. |
| `mapKey` | string | Object key of the map, by convention `hikes/{id}/map.png`. |

The offset in `start`/`end` is significant, not decoration. It is preserved through parsing and carried into the weather segment, which reports precipitation timing on the hike's local calendar day. A record written in UTC would place that day boundary in the wrong place.

`mapKey` is stored rather than derived. It lets a map live somewhere other than the conventional path — a shared map for two adjacent preserves, say — without a schema change. Because it is free-form, the object it names is confirmed to exist before a URL for it is returned.

`location_based/` holds a template per location with coordinates and `mapKey` pre-filled and the dates left as `TODO`, so publishing a hike is filling in two timestamps and a trail list. This worker never writes to the bucket.

## Retrieval

`get_hike(id)` reads `hikes/{id}.json` and distinguishes three outcomes:

- **Object absent** → `Ok(None)`, which the API surface maps to `404`. A missing hike is an ordinary answer, not an error.
- **Object present but unreadable** — no body, or JSON that does not deserialize into the record schema → `Err`. A record that exists but is malformed is a real fault and must not be reported as "no such hike", which would send the organizer looking for a missing upload instead of a broken one.
- **Object present and valid** → `Ok(Some(record))`, subject to the validation below.

Unknown fields in the JSON are ignored, so a record can carry annotations the worker does not read.

## Record Validation

A record that parses is not yet a record that can be served. Two invariants are checked before the hike leaves this segment:

| Invariant | Failure |
|---|---|
| `start` and `end` parse as RFC 3339 with offset | `Err` |
| `end` is strictly after `start` | `Err` |
| The object named by `mapKey` exists in the bucket | `Err` |

All three fail the request rather than degrading it, because each one means the record describes a hike that cannot be walked: no time, a negative duration, or no map.

The map check is an existence probe — object metadata, not bytes — issued before the URL is signed. Signing is arithmetic and would happily produce a valid-looking URL for an object that was never uploaded, leaving the client to discover the mistake as a broken image. One metadata read per hike request is a Class B operation, which the free tier has ten million of a month against a pack that hikes monthly.

The publishing path is what catches an authoring mistake early; these checks are the backstop for when it does not. A record still carrying the template's `"start": "TODO"` fails here, correctly and loudly.

## Presigned Map URLs

The map URL is an S3-compatible SigV4 query-signed `GET` against R2's `*.r2cloudflarestorage.com` endpoint, built entirely in the worker. Nothing is fetched to produce it.

| Parameter | Value |
|---|---|
| Region | `auto` |
| Service | `s3` |
| Signed headers | `host` only |
| Payload hash | `UNSIGNED-PAYLOAD` |
| TTL | 3600 seconds |
| Addressing | Path-style — bucket in the path, not the hostname |

Signing is a pure function of the current instant and the credentials, which is what makes it testable: the Workers clock is read in the adapter and passed in, so the signature logic is exercised in `cargo test` without a wasm runtime.

The response reports `map.expiresAt` as the signing instant plus the TTL, so a client that caches the URL knows when to ask again rather than discovering expiry as a fetch failure.

Percent-encoding follows SigV4's rules rather than a general URL encoder: unreserved characters pass through and everything else becomes `%XX`. Path segments of the object key are encoded individually so `/` separators survive, which matters because keys are nested.

## Configuration

| Name | Kind | Purpose |
|---|---|---|
| `HIKES` | R2 binding | Reading hike records |
| `R2_ACCOUNT_ID` | var | Presign hostname |
| `R2_BUCKET_NAME` | var | Presign path |
| `R2_ACCESS_KEY_ID` | secret | SigV4 credential |
| `R2_SECRET_ACCESS_KEY` | secret | SigV4 signing key |

The binding and the credentials are two different access paths to the same bucket: the binding reads records server-side, the credentials mint URLs the client redeems directly. Any of them failing to resolve is a `500` at admission, not a partial response.

The presign TTL is a compile-time constant. It is the kind of value that only changes if client caching behavior changes, and a var would invite per-environment drift in how long a URL stays good.

## Decisions & Alternatives

| Decision | Chosen | Alternatives Considered | Rationale |
|---|---|---|---|
| Storage | R2 objects | D1; KV; Durable Objects | One organizer, roughly one hike a month, and a single-key access pattern. A database buys a schema and a migration story for nothing, and R2 already holds the map images. |
| Record id | Location slug, no date | Date-prefixed id (`2026-09-20-blackwell`); UUID | Permanent `/hike/{id}` links across reschedules, and a bounded object count. Cost: no hike history, which the HLD accepts as a non-goal. |
| Map delivery | Presigned URL, 1-hour TTL | Proxy bytes through the worker; public bucket | A 1MB PNG through the worker would spend CPU and bandwidth on every hike view. A public bucket would make maps readable without the API key. |
| `mapKey` | Stored in the record, existence verified before signing | Derived from `id`; stored and trusted | Storing it allows a map off the conventional path without a schema change; verifying it means a missing upload surfaces as a server error naming the record, not as a broken image in the app. |
| Time-range validation | `end` must be strictly after `start` | Trust the record; clamp silently | A reversed or zero-length range yields no in-window weather periods, so without the check the symptom is a hike that mysteriously has no weather rather than a record that is wrong. |
| Presign TTL | Compile-time constant | Environment variable | Only changes if client caching behavior changes; a var invites per-environment drift. `[inferred]` |
| Missing vs. malformed record | `Ok(None)` vs. `Err` | Treat both as not-found | A malformed record is an authoring fault; reporting it as `404` sends the organizer looking for a missing upload instead of a broken one. |
| Clock injection | `now` passed into the pure signer | Read the clock inside the signer | The Workers clock cannot run under `cargo test`; injecting it keeps signature logic inside the coverage gate. |

## Open Questions & Future Decisions

### Resolved

1. ✅ One record per location, overwritten on reschedule — permanent links over hike history.
2. ✅ Presigned URLs over proxying map bytes — CPU and bandwidth are the binding constraint.
3. ✅ `mapKey`'s object is verified to exist before its URL is signed.
4. ✅ `end` must be strictly after `start`; a violation fails the request.

### Deferred

1. **A stale record is indistinguishable from a current one.** With no date in the id, a record whose `end` has passed is served as a completed hike — correctly, but silently. Whether the API should signal "this hike is over" rather than leaving the client to compare timestamps is unsettled.
2. **Nothing catches an authoring mistake at publishing time.** A record with unfilled template dates, a wrong `mapKey`, or coordinates outside NWS coverage is rejected at read time, on a request a family is waiting on. Surfacing these when the record is written belongs to the admin application and is outside this project's scope.
3. **Coordinates are not checked against NWS coverage.** A meeting point outside it fails late and softly, as a hike with no weather, in the weather segment rather than here.
4. **Map images are never invalidated.** Re-uploading a map under the same key leaves already-issued URLs pointing at the new bytes, which is usually right, but no versioning scheme exists if it ever isn't.

## References

- `scripts/upload-hike.sh` — the publishing path.
- `location_based/` — per-location record templates.
- [AWS SigV4 query signing](https://docs.aws.amazon.com/AmazonS3/latest/API/sigv4-query-string-auth.html) — the presigning algorithm.
- [R2 S3 API compatibility](https://developers.cloudflare.com/r2/api/s3/api/) — endpoint and addressing rules.
