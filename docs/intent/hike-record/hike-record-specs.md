# Hike Record — Specs

Prefix: `HIKE`. Design: [`hike-record-design.md`](hike-record-design.md).

**Verification.** A spec marked `[x]` is cited by a test carrying its id in a `@spec` annotation. Where the behaviour lives in Workers-runtime glue — `src/lib.rs`, `src/r2_adapter.rs`, `src/weather_adapter.rs`, none of which can execute under `cargo test` — the citing test is an assertion in `scripts/smoke-test.sh`, which runs against the deployed preview worker. Specs describing layout conventions or structural properties rather than triggerable behaviour are cited at the code that embodies them and have no test of their own.

## Object layout

- [x] **HIKE-OBJ-001**: The system shall read hike records from an R2 bucket it never writes to, treating the sibling admin worker as that bucket's only writer.
- [x] **HIKE-OBJ-002**: The system shall expect one hike record per location, keyed by the location slug with no date component and overwritten when that location is next scheduled, so that a hike's URL is stable across reschedules.
- [x] **HIKE-OBJ-003**: The system shall address each hike record as the R2 object `hikes/{id}.json`, where `{id}` is the location slug requested.
- [x] **HIKE-OBJ-004**: The system shall expect a location's trail map at `hikes/{id}/map.png` by convention, while resolving the map through the record's `mapKey`, which may name any object key in the bucket.

## Record retrieval

- [x] **HIKE-REC-001**: When a hike is requested by id, the system shall read the R2 object `hikes/{id}.json` through the `HIKES` binding.
- [x] **HIKE-REC-002**: If no object exists at `hikes/{id}.json`, then the system shall report the hike as absent rather than as an error.
- [x] **HIKE-REC-003**: If the object at `hikes/{id}.json` carries no body, or does not deserialize into the hike record schema, then the system shall report an error rather than reporting the hike as absent.
- [x] **HIKE-REC-004**: The system shall ignore fields present in the record JSON that the hike record schema does not define.
- [x] **HIKE-REC-005**: The system shall read `id`, `meeting.lat`, `meeting.lon`, `trails`, and `mapKey` from every hike record, and `start`/`end` when present, so that a record written with no date still deserializes.

## Location list

- [x] **HIKE-LOC-001**: When the location list is requested, the system shall read the R2 object `resources/hike-locations.json` through the `HIKES` binding.
- [x] **HIKE-LOC-002**: If no object exists at `resources/hike-locations.json`, then the system shall report the location list as absent rather than as an error.
- [x] **HIKE-LOC-003**: If the object at `resources/hike-locations.json` carries no body, or does not deserialize into a JSON array of objects each carrying string `short_name` and `full_name` fields, then the system shall report an error rather than reporting the list as absent.
- [x] **HIKE-LOC-004**: When the location list is read, the system shall return its entries in stored order.
- [x] **HIKE-LOC-005**: The system shall accept an empty array at `resources/hike-locations.json` as a valid, empty location list.
- [x] **HIKE-LOC-006**: The system shall ignore fields on a location-list entry other than `short_name` and `full_name`.

## Record validation

- [x] **HIKE-REC-006**: When serving API version 2, if a hike record's `start` or `end` is absent or does not parse as an RFC 3339 timestamp carrying a UTC offset, then the system shall report an error rather than serving the hike. API version 3 never reads the record's `start`/`end` and so is unaffected by either (`api:API-WIN-004`, `api:API-WIN-005`).
- [x] **HIKE-REC-007**: Where a hike record carries both `start` and `end`, if `end` is not strictly after `start`, then the system shall report an error rather than serving the hike.
- [x] **HIKE-REC-008**: The system shall validate a hike record's timestamps during retrieval, so that no consumer receives a record it must itself parse or check.
- [x] **HIKE-REC-009**: The system shall return each retrieved hike record with its `start` and `end` already parsed, retaining the UTC offset each was written with, where either is present.

## Map presigning

- [x] **HIKE-MAP-001**: When a validated hike record is served, the system shall return a presigned URL for the R2 object named by that record's `mapKey`.
- [x] **HIKE-MAP-002**: Before returning a presigned map URL, the system shall confirm through an object-metadata read that the object named by `mapKey` exists in the bucket.
- [x] **HIKE-MAP-009**: If the object named by a record's `mapKey` does not exist, then the system shall report the map as absent rather than reporting an error, leaving the response version to decide how to express that.
- [x] **HIKE-MAP-003**: The system shall presign map URLs using AWS Signature Version 4 query signing against the R2 S3-compatible endpoint, with region `auto`, service `s3`, `host` as the only signed header, and an unsigned payload.
- [x] **HIKE-MAP-004**: The system shall address the bucket path-style when presigning, placing the bucket name in the URL path rather than the hostname.
- [x] **HIKE-MAP-005**: The system shall presign map URLs with a time to live of 3600 seconds.
- [x] **HIKE-MAP-006**: The system shall report `map.expiresAt` as the signing instant plus the presign time to live.
- [x] **HIKE-MAP-007**: The system shall percent-encode object keys for signing per Signature Version 4 rules, encoding each path segment individually so that `/` separators are preserved.
- [x] **HIKE-MAP-008**: The system shall compute a presigned URL as a pure function of a supplied signing instant, so that signing is exercised in tests without the Workers runtime clock.

## Configuration

- [x] **HIKE-CFG-001**: The system shall read `R2_ACCOUNT_ID` and `R2_BUCKET_NAME` from environment variables, and `R2_ACCESS_KEY_ID` and `R2_SECRET_ACCESS_KEY` from secrets.
- [x] **HIKE-CFG-002**: The system shall read hike records and the location list through the `HIKES` R2 binding and mint presigned map URLs through the R2 API credentials.
