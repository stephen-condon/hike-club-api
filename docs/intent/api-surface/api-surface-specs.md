# API Surface — Specs

Prefix: `API`. Design: [`api-surface-design.md`](api-surface-design.md).

**Verification.** A spec marked `[x]` is cited by a test carrying its id in a `@spec` annotation. Where the behaviour lives in Workers-runtime glue — `src/lib.rs`, `src/r2_adapter.rs`, `src/weather_adapter.rs`, none of which can execute under `cargo test` — the citing test is an assertion in `scripts/smoke-test.sh`, which runs against the deployed preview worker. Specs describing layout conventions or structural properties rather than triggerable behaviour are cited at the code that embodies them and have no test of their own.

## Routing

- [x] **API-ROUTE-001**: The system shall serve `GET /health` without an API key or version header, responding 200 with the body `ok`.
- [x] **API-ROUTE-002**: If a request names a path the system does not route, then the system shall respond 501 with `not implemented`.
- [x] **API-ROUTE-003**: If a request names a routed path with a method that path does not support, then the system shall respond 405 with `method not allowed` and an `Allow` header naming the supported methods.
- [x] **API-ROUTE-004**: When a request names `/hike` or `/hike/` with no hike id, the system shall respond 404 with `hike not found`.
- [x] **API-ROUTE-005**: The system shall answer an unrouted path or unsupported method before evaluating the API key, so that routing outcomes do not depend on authentication.

## Admission

- [x] **API-AUTH-001**: If the `API_KEY` secret cannot be read, then the system shall respond 500 with `server misconfigured: API_KEY not set` before evaluating any request header.
- [x] **API-AUTH-002**: The system shall require both `GET /hike/{id}` and `GET /hike-locations` to carry an `x-api-key` header equal to the `API_KEY` secret.
- [x] **API-AUTH-003**: If the `x-api-key` header is absent or does not equal the `API_KEY` secret, then the system shall respond 401 with `unauthorized` and no further detail.
- [x] **API-AUTH-004**: The system shall evaluate API-key authorization before version negotiation, before hike identity, and before resolving storage configuration.
- [x] **API-ERR-001**: If the R2 configuration variables, the R2 secrets, or the `HIKES` binding cannot be resolved, then the system shall respond 500 with `server misconfigured: {detail}`.

## Version negotiation

- [x] **API-VER-001**: The system shall require every request to `GET /hike/{id}` and `GET /hike-locations` to carry an `x-api-version` header, with no default applied when it is absent.
- [x] **API-VER-002**: If the `x-api-version` header is absent, names a version the system does not know, or is not an integer, then the system shall respond 400 with `unsupported api version`.
- [x] **API-VER-003**: The system shall serve `GET /hike-locations` with an identical payload under every supported version, while still rejecting an unsupported version per API-VER-002.
- [x] **API-VER-004**: The system shall hold a version registry recording, for each known API version, whether it is current, deprecated with a sunset date, or already sunset.
- [x] **API-VER-005**: While the requested API version's sunset date is at or before the current time, the system shall respond 410 with a body naming that version, its sunset date, and the API versions not past their sunset.
- [x] **API-VER-006**: While the requested API version is deprecated and its sunset date is in the future, the system shall serve the response in full and stamp `Deprecation: true`, a `Sunset` header carrying that version's date, and a `Link` header referencing the OpenAPI document with `rel="deprecation"`.
- [x] **API-VER-007**: The system shall stamp `Deprecation`, `Sunset`, and `Link` headers only on successful responses, and on no error response.
- [x] **API-VER-008**: The system shall record API version 1 as sunset on `Thu, 20 Aug 2026 00:00:00 GMT`, API version 2 as deprecated with a sunset of `Wed, 18 Nov 2026 00:00:00 GMT`, and API version 3 as current.
- [x] **API-VER-009**: The system shall keep a sunset API version in the version registry after its response shape is removed, so that a request naming it answers 410 rather than the 400 given to an unknown version.
- [x] **API-VER-010**: The system shall evaluate a version's sunset against a current time supplied to it, so that sunset enforcement is exercised in tests without the Workers runtime clock.

## Location mapping

- [ ] **API-LOC-001**: When `GET /hike-locations` is admitted and the location list reads successfully, the system shall respond 200 with that list as a JSON array of objects carrying `short_name` and `full_name`, in stored order.
- [x] **API-LOC-002**: The system shall serve `GET /hike-locations` with a `content-type` of `application/json`.
- [ ] **API-LOC-004**: The system shall read the location list for `GET /hike-locations` only after API-key authorization and version negotiation have admitted the request.
- [ ] **API-LOC-005**: If the location list is absent from the bucket, then `GET /hike-locations` shall respond 500 with `server misconfigured: location list not found`.
- [ ] **API-LOC-006**: If the location list is present but cannot be read, then `GET /hike-locations` shall respond 502 with `upstream error: {detail}`.
- [ ] **API-LOC-007**: The `GET /hike-locations` response shall contain only the location list read from the bucket, with no list compiled into the worker and no substitute list served when the read fails.
- [ ] **API-LOC-008**: `openapi.yaml` shall document the `GET /hike-locations` 500 and 502 responses, and shall name a hike's location slug by the wire field `short_name`.

## Response assembly

- [x] **API-RESP-001**: When a hike record is found and validated, the system shall respond 200 with the hike's id, start, end, meeting point, trails, map reference, weather availability flag, and weather block.
- [x] **API-RESP-002**: The system shall echo `start` and `end` into the response verbatim from the hike record, preserving the UTC offset as written.
- [x] **API-RESP-003**: The system shall render `meetingPoint` as the record's latitude and longitude plus a `googleMapsUrl` of the form `https://maps.google.com/?q={lat},{lon}`.
- [x] **API-RESP-004**: If no hike record exists for the requested id, then the system shall respond 404 with `hike not found`.
- [x] **API-RESP-005**: If the weather segment does not produce a weather block for a hike, then the system shall respond 200 with `weatherAvailable: false` and a null `weather` block, without itself inspecting the underlying forecast.
- [x] **API-RESP-006**: The system shall set `weatherAvailable` to true if and only if the response carries a weather block.
- [x] **API-RESP-007**: If hike-record retrieval, hike-record validation, or map presigning fails, then the system shall respond 502 with `upstream error: {detail}`.
- [ ] **API-RESP-010**: If the trail map object cannot be confirmed to exist, then under API version 3 the system shall respond 200 with `mapAvailable: false` and a null `map`.
- [x] **API-RESP-011**: If the trail map object cannot be confirmed to exist, then under API version 2 the system shall respond 502, because version 2's response shape cannot express an absent map.
- [x] **API-RESP-008**: The system shall pass the UTC offset written in the hike record's `start` to the weather segment, so that precipitation timing and observation caching are both computed on the hike's local calendar day.
- [x] **API-RESP-009**: The system shall depend on hike storage and on weather only through the `HikeStore` and `WeatherSource` abstractions, so that response assembly is exercised in tests without network access or the Workers runtime.

## Wire contract

- [x] **API-WIRE-001**: The system shall render the response envelope — `id`, `start`, `end`, `meetingPoint`, `trails`, `map`, `weatherAvailable`, `weather` — under every API version it serves.
- [x] **API-WIRE-003**: Under API version 2 the system shall render the weather block with `startTempF`, `endTempF`, `conditions`, `precipitation` carrying `probabilityPct`, `expected`, `startsAt`, and `endsAt`, plus `heatIndexF`, `windChillF`, and `alerts`.
- [ ] **API-WIRE-004**: Under API version 3 the system shall render the weather block as API version 2's, replacing `conditions` with `startConditions` and `endConditions`.
- [ ] **API-WIRE-009**: Under API version 3 the system shall render `map` as nullable and add a `mapAvailable` flag to the envelope.
- [x] **API-WIRE-005**: The system shall render `map`, when present, as a presigned URL and an `expiresAt` timestamp in RFC 3339.
- [x] **API-WIRE-006**: The system shall render error response bodies as plain text rather than JSON.
- [x] **API-WIRE-007**: The system shall validate the serialized response of every API version it serves against the schema published in `openapi.yaml` as part of its test suite, without a deployed worker.
- [ ] **API-WIRE-008**: The system shall publish the API version 3 response schema in `openapi.yaml` and remove the API version 1 schema.
- [x] **API-WIRE-010**: The system shall carry no response types, builders, or contract tests for an API version past its sunset.
