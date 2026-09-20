# API Surface — Specs

Prefix: `API`. Design: [`api-surface-design.md`](api-surface-design.md).

## Routing

- [x] **API-ROUTE-001**: The system shall serve `GET /health` without an API key or version header, responding 200 with the body `ok`.
- [ ] **API-ROUTE-002**: If a request names a path the system does not route, then the system shall respond 501 with `not implemented`.
- [ ] **API-ROUTE-003**: If a request names a routed path with a method that path does not support, then the system shall respond 405 with `method not allowed` and an `Allow` header naming the supported methods.
- [ ] **API-ROUTE-004**: When a request names `/hike` or `/hike/` with no hike id, the system shall respond 404 with `hike not found`.

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
- [ ] **API-VER-004**: The system shall hold a version registry recording, for each known API version, whether it is current, deprecated with a sunset date, or already sunset.
- [ ] **API-VER-005**: While the requested API version's sunset date is at or before the current time, the system shall respond 410 with a body naming that version, its sunset date, and the API versions not past their sunset.
- [x] **API-VER-006**: While the requested API version is deprecated and its sunset date is in the future, the system shall serve the response in full and stamp `Deprecation: true`, a `Sunset` header carrying that version's date, and a `Link` header referencing the OpenAPI document with `rel="deprecation"`.
- [x] **API-VER-007**: The system shall stamp `Deprecation`, `Sunset`, and `Link` headers only on successful responses, and on no error response.
- [ ] **API-VER-008**: The system shall record API version 1 as sunset on `Thu, 20 Aug 2026 00:00:00 GMT`, API version 2 as deprecated with a sunset of `Wed, 18 Nov 2026 00:00:00 GMT`, and API version 3 as current.

## Location mapping

- [x] **API-LOC-001**: When `GET /hike-locations` is served, the system shall respond with the slug-to-display-name mapping embedded in the binary at compile time, verbatim.
- [x] **API-LOC-002**: The system shall serve `GET /hike-locations` with a `content-type` of `application/json`.
- [x] **API-LOC-003**: The system shall serve `GET /hike-locations` without reading R2, so that the endpoint has no storage failure mode.

## Response assembly

- [x] **API-RESP-001**: When a hike record is found and validated, the system shall respond 200 with the hike's id, start, end, meeting point, trails, map reference, weather availability flag, and weather block.
- [x] **API-RESP-002**: The system shall echo `start` and `end` into the response verbatim from the hike record, preserving the UTC offset as written.
- [x] **API-RESP-003**: The system shall render `meetingPoint` as the record's latitude and longitude plus a `googleMapsUrl` of the form `https://maps.google.com/?q={lat},{lon}`.
- [x] **API-RESP-004**: If no hike record exists for the requested id, then the system shall respond 404 with `hike not found`.
- [x] **API-RESP-005**: If the weather source fails, or succeeds but yields no periods covering the hike window, then the system shall respond 200 with `weatherAvailable: false` and a null `weather` block.
- [x] **API-RESP-006**: The system shall set `weatherAvailable` to true if and only if the response carries a weather block.
- [x] **API-RESP-007**: If hike-record retrieval, hike-record validation, or map presigning fails, then the system shall respond 502 with `upstream error: {detail}`.
- [x] **API-RESP-008**: The system shall pass the UTC offset written in the hike record's `start` to the weather segment, so that API versions 2 and 3 compute precipitation timing on the hike's local calendar day.
- [x] **API-RESP-009**: The system shall depend on hike storage and on weather only through the `HikeStore` and `WeatherSource` abstractions, so that response assembly is exercised in tests without network access or the Workers runtime.

## Wire contract

- [x] **API-WIRE-001**: The system shall keep the response envelope — `id`, `start`, `end`, `meetingPoint`, `trails`, `map`, `weatherAvailable`, `weather` — identical across every API version.
- [x] **API-WIRE-002**: Under API version 1 the system shall render the weather block with `temperatureF`, `conditions`, `precipitation` carrying `probabilityPct` and `amountIn`, `heatIndexF`, `windChillF`, and `alerts`.
- [x] **API-WIRE-003**: Under API version 2 the system shall render the weather block with `startTempF`, `endTempF`, `conditions`, `precipitation` carrying `probabilityPct`, `expected`, `startsAt`, and `endsAt`, plus `heatIndexF`, `windChillF`, and `alerts`.
- [ ] **API-WIRE-004**: Under API version 3 the system shall render the weather block as API version 2's, replacing `conditions` with `startConditions` and `endConditions`.
- [x] **API-WIRE-005**: The system shall render `map` as a presigned URL and an `expiresAt` timestamp in RFC 3339.
- [x] **API-WIRE-006**: The system shall render error response bodies as plain text rather than JSON.
- [x] **API-WIRE-007**: The system shall validate each API version's serialized response against the schema published in `openapi.yaml` as part of its test suite, without a deployed worker.
- [ ] **API-WIRE-008**: The system shall publish the API version 3 response schema in `openapi.yaml`.
