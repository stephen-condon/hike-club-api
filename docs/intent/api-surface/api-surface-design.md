---
parent: high-level-design
prefix: API
---

# API Surface

## Context and Design Philosophy

Everything between an inbound HTTP request and a serialized JSON response: routing, admission control, version negotiation, orchestration of the two data segments, and the wire contract itself.

The guiding principle is that admission is cheap and total. A request is rejected for a missing key or an unusable version header before any R2 object is read or any NWS call is made, so rejected traffic costs one comparison. Past admission, the response is assembled from whatever the data segments return — including their failures — rather than being abandoned when one of them fails.

The segment depends on `hike-record` and `weather` only through the `HikeStore` and `WeatherSource` traits. Orchestration is generic over both, which is what lets the whole request path be unit-tested with fixtures and no network.

## Endpoints

| Endpoint | Auth | Version header | Response |
|---|---|---|---|
| `GET /health` | none | not read | `200 ok` (text) |
| `GET /hike/{id}` | required | required | `200` hike object in the requested version's shape |
| `GET /hike-locations` | required | required | `200` location list (identical under every version) |

`404` means one thing and one thing only: *that hike does not exist*. An unrouted path is `501 Not Implemented` and a known path under an unsupported method is `405 Method Not Allowed` with an `Allow` header, so a client distinguishing "wrong URL" from "no such hike" never has to guess. Paths under `/hike/` that name no id — `/hike` and `/hike/` — resolve to `404` rather than `501`: they address the hike collection correctly and simply identify nothing in it.

`/health` is deliberately open: it is a liveness probe with no data in it, and requiring a key would mean the probe fails for the one reason a probe must not fail — a misconfigured secret.

`/hike-locations` reads the version header and rejects a bad one even though its payload does not vary by version. A client that would be served a broken hike response should not be told its version is fine by a sibling endpoint.

## Location Mapping

`GET /hike-locations` serves the list of `{short_name, full_name}` entries the app uses to populate its picker. It reads the list through `HikeStore::get_locations` on every request, after admission, and serializes what it gets back as a JSON array in stored order.

The endpoint has no fallback. An absent list is a `500`, because the bucket was never seeded and that is a deployment fault; an unreadable one is a `502`, because the object exists and is wrong. Neither is ever answered with a substitute list: the app keeps its cached locations when a refresh fails but replaces them with any list it is served, so an error costs the organizer nothing while a wrong list silently corrupts the picker. The response is uncached; the app refreshes weekly.

## Request Admission

Routing resolves before admission begins: an unrouted path and a routed path under an unsupported method are answered without the key being read, so a `501` or a `405` says the same thing to an authenticated client and an anonymous one.

Admission itself runs in a fixed order, and the order is load-bearing:

1. **Server configuration.** The `API_KEY` secret must be readable; if it is not, the request fails `500` before anything is compared. A worker with no key configured must not accidentally admit traffic.
2. **API key.** The `x-api-key` header must equal `API_KEY`. A missing header and a wrong key are the same outcome — `401`, no detail.
3. **Version.** `x-api-version` must be present and one of the supported values.
4. **Hike identity.** An absent or empty id is not a malformed request; it is a request for a hike that does not exist, and answers `404`.
5. **Data-segment configuration.** R2 variables, secrets, and the `HIKES` binding must all resolve.

Key comparison is ordinary equality rather than constant-time. The header is a single shared secret behind a Cloudflare WAF rule that already drops unauthenticated traffic at the edge, not a signature being verified against attacker-chosen input, so timing analysis has no useful target.

## Version Negotiation and Deprecation

`x-api-version` carries a bare integer naming a version in the registry below; a missing header, an unknown number, and a non-integer are all rejected identically with `400`. There is no default version.

A version registry maps each version to a sunset date or to nothing:

| Version | Status | Sunset | Response shape |
|---|---|---|---|
| 1 | Sunset | `Thu, 20 Aug 2026 00:00:00 GMT` | none — removed |
| 2 | Deprecated | `Wed, 18 Nov 2026 00:00:00 GMT` | frozen |
| 3 | Current | — | current |

A sunset version stays in the registry after its shape is deleted. The entry is what separates "this version is finished" from "this version never existed": without it, a stale client asking for version 1 would get the `400` meant for a typo instead of the `410` that tells it what to do.

A sunset date is enforced, not advertised. A version whose date has passed is `410 Gone`, and the body names the versions still being served, so a stale app build learns both that it is finished and what to ask for instead. Advertising a date and then serving the version past it trains clients to ignore the header, which is the failure the mechanism exists to prevent.

A version that is deprecated but not yet sunset is served in full, and carries RFC 8594 headers:

| Header | Value |
|---|---|
| `Deprecation` | `true` |
| `Sunset` | the registry's HTTP-date for that version |
| `Link` | the OpenAPI document, `rel="deprecation"` |

These headers are stamped on successful responses. An error response does not carry them — a client debugging a `401` is not being told about a sunset, and a `400` for an unsupported version has no version to describe. The `410` is the exception in substance rather than form: it carries no `Sunset` header because the sunset is the entire message, spelled out in the body.

Parsing and the registry are pure functions held apart from the Workers glue that reads and writes the actual headers, so both are unit-tested.

## Response Assembly

Assembly is one pure async function over both traits:

1. Fetch the hike record. Absent → the whole request is a `404`. The record arrives validated, with `start` and `end` already parsed; an invalid record surfaces here as a failure, not later as a confusing symptom.
2. Confirm the map object exists and presign its URL, capturing the expiry.
3. Request weather for the meeting coordinates over the hike window, passing the hike's UTC offset. A failure here is captured, not propagated.
4. Build the version-appropriate response.

The hike's UTC offset crosses into the weather segment because two of that segment's behaviours need it: precipitation timing is reported on the hike's local calendar day, and observations are cached under that same day. Both are statements about the hiker's day, not about UTC, so the offset travels with the request rather than being re-derived.

Weather and, under v3, the trail map are allowed to fail without failing the request. When weather does not produce a block the response carries `weatherAvailable: false` and a null `weather`; when v3 cannot confirm a map object the response carries `mapAvailable: false` and a null `map`. Everything else — the record itself, its timestamps, its meeting point — ends the request when it fails, because a hike screen cannot be assembled without any of it.

A hike whose map upload failed still gets the pack to the trailhead. Withholding the meeting point over a missing PNG trades a degraded screen for no screen at all.

## Wire Contract

Both versions share the same envelope: `id`, `start`, `end`, `meetingPoint`, `trails`, `map`, `weatherAvailable`, `weather`. `start` and `end` are echoed from the record verbatim, offset intact, so the client renders local time without knowing the preserve's timezone. `meetingPoint` carries the raw coordinates plus a pre-built `maps.google.com` URL, so the app does not compose one.

The versions differ only in the `weather` block:

| | v2 `WeatherV2` (frozen) | v3 `WeatherV3` |
|---|---|---|
| Temperature | `startTempF` / `endTempF` | `startTempF` / `endTempF` |
| Conditions | `conditions` — first in-window period | `startConditions` / `endConditions` |
| Precipitation | `probabilityPct`, `expected`, `startsAt`, `endsAt` | same |
| NWS alerts | only alerts overlapping the hike window | same |

v3 exists because conditions and temperature describe the same span and should be read the same way. A hike that starts sunny and ends in thunderstorms is labelled "Sunny" under v2; pairing conditions with the temperatures already reported at both ends of the window makes the block internally consistent. The change renames a field, which no shipped version may do, so it lands in a new version and v2 freezes.

v3 also makes `map` nullable, with a `mapAvailable` flag beside it, mirroring how weather already degrades. Under v2 a missing map image is still a `502`, because v2's shape cannot express an absent map and a frozen shape is not corrected in place.

`weatherAvailable` restates whether `weather` is null. It stays because a client reading a boolean is less likely to mishandle the absent case than one testing a nested object for null, and removing it would be a breaking change to both shipped versions.

`openapi.yaml` is the published contract. Conformance is proven in-process: the contract test serializes each response type and validates the JSON against schemas extracted from the OpenAPI document, translating OpenAPI's `nullable: true` into the `anyOf`-with-null form a JSON Schema validator understands. This catches a Rust type drifting from the published spec at push time, with no deployed worker.

## Error Mapping

| Condition | Status | Body |
|---|---|---|
| Unrouted path | 501 | `not implemented` |
| Known path, unsupported method | 405 | `method not allowed` (with `Allow`) |
| `API_KEY` secret unreadable | 500 | `server misconfigured: API_KEY not set` |
| Missing or wrong `x-api-key` | 401 | `unauthorized` |
| Missing or unsupported `x-api-version` | 400 | `unsupported api version` |
| Requested version is past its sunset | 410 | `api version {n} was sunset on {date}; supported versions: {list}` |
| Absent or empty hike id, or no record for that id | 404 | `hike not found` |
| R2 configuration or binding unresolvable | 500 | `server misconfigured: {detail}` |
| Record unusable, or assembly failed | 502 | `upstream error: {detail}` |
| Location list absent | 500 | `server misconfigured: location list not found` |
| Location list unreadable | 502 | `upstream error: {detail}` |

Bodies are plain text, not JSON. The sole client renders a generic failure state; a structured error object would be contract surface maintained for nobody.

## Decisions & Alternatives

| Decision | Chosen | Alternatives Considered | Rationale |
|---|---|---|---|
| Version transport | Required `x-api-version` header | URL path prefix (`/v2/hike/{id}`); `Accept` media type; defaulting to the oldest version | A header keeps one route per resource and makes the version a cross-cutting concern the router does not model. Requiring it prevents a client from being silently pinned to the oldest shape forever, which would make the sunset date unenforceable. |
| Key comparison | Plain equality | Constant-time comparison | Shared secret behind an edge WAF rule, not a signature check; timing analysis has no target. `[inferred]` |
| `/health` auth | Unauthenticated | Same key as other endpoints | A liveness probe that fails when a secret is misconfigured cannot distinguish "down" from "misconfigured". `[inferred]` |
| Version check on `/hike-locations` | Enforced despite version-independent payload | Skip negotiation entirely | A client with a bad version header should learn it from any endpoint, not just the one whose shape varies. |
| Weather failure handling | Degrade to `weatherAvailable: false` | Fail the request; serve stale weather | The meeting point and map are what a family needs on arrival; an NWS outage must not hide them. |
| Error body format | Plain text | JSON error envelope | One client, which renders a generic failure state. A structured envelope would be contract surface with no consumer. `[inferred]` |
| Contract testing | In-process schema conformance | Live calls against a deployed worker | Catches the failure that matters — types drifting from the published spec — at push time with no network, no deploy, and no flake. |
| Deprecation headers on errors | Not stamped | Stamp on every response | An error response is not a served version; a `400` for a bad version has no version to describe. `[inferred]` |
| Unrouted path | `501`, not `404` | `404` for anything unmatched | `404` is reserved for "that hike does not exist". Overloading it with "that URL does not exist" makes a client's two most likely mistakes indistinguishable. |
| Routing versus admission order | An unrouted path answers before the API key is checked | Authenticate first, so route existence is never disclosed | Route shape is not a secret worth a branch: the endpoints are published in `openapi.yaml` and the WAF is the boundary that matters. |
| Sunset version after its shape is deleted | Kept in the registry | Removed with the shape | The entry is what distinguishes a finished version from a nonexistent one, and so what makes `410` reachable rather than `400`. |
| Missing map under a frozen version | `502` under v2, degraded under v3 | Degrade under both; fail under both | v2's shape has no way to say "no map", and a frozen shape is not corrected in place. |
| Empty hike id | `404` | `400 missing hike id` | `/hike/` addresses the collection correctly and names nothing in it — a hike that does not exist, not a malformed request. |
| Past-sunset version | `410` naming the supported versions | Keep serving it; `400` | A date that is advertised but never enforced teaches clients to ignore the header. Naming the live versions in the body means a stale build learns what to ask for without a doc lookup. |
| Location list failure | `500` when absent, `502` when unreadable, no fallback | Serve an embedded copy; serve an empty list; one status for both | The app keeps its cache when a refresh fails and overwrites it when one succeeds, so any substitute list does more harm than an error. Absent and unreadable get different statuses because they have different fixes: seed the bucket versus correct the object. |
| Conditions paired with temperatures | New v3 | Add `endConditions` to v2; rename in place in v2 | Renaming a field breaks a shipped client, which no version may do. The asymmetry of `startTempF`/`endTempF` beside `conditions`/`endConditions` would outlive the reason for it. |

## Open Questions & Future Decisions

### Resolved

1. ✅ Version is required rather than defaulted — a default makes the sunset unenforceable.
2. ✅ Weather degrades; everything else fails the request.
3. ✅ Sunset dates are enforced with `410`, not merely advertised.
4. ✅ `404` means "no such hike" exclusively; unrouted paths are `501`.
5. ✅ An unusable record yields `502 upstream error`. It reads as an upstream fault rather than an authoring one, but the operator's signal for a bad record is the publishing path, not the response code.

### Deferred

1. **No cache headers on hike responses.** Every request re-reads R2 and re-presigns. Whether a short `Cache-Control` is worth the staleness after a reschedule is open.
2. **v2's sunset is sixty days out.** `Wed, 18 Nov 2026` gives the app one release cycle to reach v3. Whether sixty days is the standing convention for future deprecations or a one-off for this transition is unsettled.
3. **Nothing tells a client that a version is nearing sunset except the headers it may not read.** The `410` is the first hard signal, and by then the app is broken in the field.

## References

- `openapi.yaml` — the published contract for every served version.
- [RFC 8594](https://www.rfc-editor.org/rfc/rfc8594) — the `Sunset` HTTP header.
- `docs/intent/hike-record/hike-record-design.md` — the `HikeStore` side of the contract.
- `docs/intent/weather/weather-design.md` — the `WeatherSource` side, and the versioned weather blocks.
