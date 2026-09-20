---
parent: high-level-design
prefix: WX
---

# Weather

## Context and Design Philosophy

The pack does not need a forecast; it needs to know whether to bring rain gear, whether it will be dangerously hot, and whether the county is under a warning. This segment turns National Weather Service data into those conclusions.

Three principles shape it. **Interpretation happens here, not in the app** — the client receives temperatures, a condition phrase, and a list of alerts already decided, because a phone in a parking lot is a bad place to reason about hourly probability arrays. **The whole segment is best-effort** — every failure path ends in "no weather" rather than an error, because the API surface treats weather as droppable. And **an absent answer is never cached as a negative one**, because caching an upstream gap would turn a thirty-second outage into ten minutes of a hike screen claiming there is no weather.

Fetching, caching, and parsing are separated from interpretation: the runtime adapter produces a normalized forecast, and pure builders turn it into version-specific output. Only the adapter touches the network, which is why it sits outside the coverage gate and everything below it sits inside.

## Source Selection

The hike window decides which NWS product answers:

- **`end` is in the past** → observations from the nearest reporting station. The hourly forecast is future-only, so a completed hike would otherwise have no weather at all, and a screen that goes blank the afternoon of the hike is worse than one that reports what happened.
- **Otherwise** → the hourly forecast, plus active alerts for the point.

Observations are reshaped into the same period type the forecast produces, so every builder downstream is unaware of which source it is working from.

## NWS Interaction

All requests carry a `User-Agent` identifying the application and a contact address, which the NWS API requires.

**Forecast path.** `/points/{lat},{lon}` yields the point's `forecastHourly` URL; that URL yields hourly periods. `/alerts/active?point={lat},{lon}` yields active watches, warnings, and advisories with their validity windows.

**Observation path.** `/points/{lat},{lon}` yields the point's `observationStations` URL; that list is proximity-ordered. The nearest station is tried first, and a station that returns no readings for the window is passed over for the next one, up to three stations. Exhausting all three is a conclusive answer — nothing was recorded near this hike — and is cached as such. A station being listed for a point does not mean it was reporting that day — equipment goes offline — and stopping at the first one turns a single silent gauge into a hike with no weather. The cap bounds the latency a fully dead neighbourhood of stations can cost, since each attempt is a round trip against a 1s P90 budget.

Coordinates are formatted to four decimal places — about eleven metres, far finer than a forecast grid cell, and stable enough that the same meeting point always produces the same URL. The same precision is used for cache keys, so a key and the request it stands for describe the same point.

Observation query timestamps use the `Z` form rather than a numeric offset. An unencoded `+` in a query string decodes as a space, which NWS answers with a `400`.

**Any response with status 400 or above is a failure**, even though NWS returns a JSON body with its errors. Parsing that body would yield an empty forecast indistinguishable from a real one, which the caching layer would then store as an authoritative "no weather".

## Caching

Read-through against the Workers Cache API, keyed by synthetic URLs:

| Kind | Key | TTL |
|---|---|---|
| Forecast | `https://cache.internal/weather?lat={lat:.4}&lon={lon:.4}` | 600 s |
| Observations | `https://cache.internal/observed?lat={lat:.4}&lon={lon:.4}&day={local date}` | 86400 s |

Keys carry the same four-decimal coordinates the NWS requests use, so one key never stands for two different upstream queries. A preserve's meeting point is fixed in its record, so in practice every request for a location shares one entry regardless.

Forecasts expire in ten minutes because NWS updates hourly and a stale forecast is a wrong answer about the future. Observations get a day because a past hike's weather never changes.

The forecast key deliberately omits the hike window: the whole hourly forecast for a point is cached once and each request filters it to its own window. The observation key includes the hike's date, because each past hike queries a different range — and that date is the hike's **local** calendar day, the same day boundary the precipitation timing uses, so the segment has one definition of "day" rather than two.

**A periodless forecast is never written to the cache, and a periodless forecast entry read from the cache is treated as a miss.** An empty forecast means upstream had nothing to say — a gap, an error body — not that there is no weather. Writing one would suppress weather for the full TTL; the guard on the read side also self-heals entries written before the rule existed.

Observations are the opposite case, and are cached even when empty. A completed hike whose three nearest stations all reported nothing is a settled fact, not a transient gap: the day is over and no reading will appear later. Refusing to cache it would mean every view of that hike pays the full station walk — four upstream requests — for an answer that cannot change. The distinction is not emptiness but whether the underlying question is still open.

## Parsing

Parsing is total and lenient: a record that cannot be understood is dropped, never fatal, so one malformed hour does not cost the whole forecast.

**Forecast periods** need a parseable start, end, and temperature; anything else is skipped. Relative humidity and wind may be absent. Wind arrives as prose (`"10 mph"`, `"10 to 20 mph"`); the first number is taken, so a range is read as its low end. Precipitation probability defaults to zero when absent, and the condition phrase to empty.

**Observations** need a timestamp and a temperature. Temperature arrives in Celsius and is converted. Wind is tagged with a unit code — metres per second when the code says so, otherwise kilometres per hour, which is the NWS default and the safer assumption for an unrecognized code. `precipitationLastHour` is frequently null; any positive value is recorded as 100% and anything else as 0, turning an observed amount into the same probability field the forecast populates.

Each observation is a point-in-time reading given a one-hour span, since observations arrive roughly hourly, so window filtering includes readings that bracket the hike. NWS returns observations newest-first; they are sorted ascending to match forecast ordering, on which the builders' "first period" and "last period" depend.

**Alerts** need an event name. Validity bounds come from `onset`/`ends`, falling back to `effective`/`expires` — NWS populates one pair or the other. A missing bound stays absent and is treated as open-ended.

## Window Filtering

A period is in the hike window when it overlaps it — `period.start < hike.end && period.end > hike.start`. Overlap rather than containment means a hike starting at 08:30 sees the 08:00–09:00 hour, which is the hour it is actually walking through.

When no period overlaps the window, there is no weather block at all. This is the ordinary outcome for a hike beyond forecast range.

## Derived Signals and Thresholds

| Quantity | Computed from | Gate | Alert when |
|---|---|---|---|
| Precipitation probability | Maximum across in-window periods | — | ≥ 1% |
| Heat index | NWS Rothfusz regression over in-window periods | Periods at or above 80°F, humidity present | > 85°F |
| Wind chill | NWS wind chill formula over in-window periods | Periods at or below 50°F, wind above 3 mph | < 32°F |

The gates are the formulas' own domains, not extra caution: Rothfusz is undefined below roughly 80°F and the wind chill formula below about 3 mph. Reporting a number outside its domain would be inventing one. When no period passes the gate the value is absent rather than zero, and an absent value raises no alert.

Heat index is reported as the worst (highest) in-window value and wind chill as the worst (lowest), because a hike leader is deciding against the hardest moment of the hike, not its average.

The 1% precipitation threshold makes any nonzero chance worth mentioning. Rain on a hike with eight-year-olds is a gear decision, and the cost of an unnecessary poncho is lower than the cost of a soaked pack.

## Alert Composition

Alerts are emitted in a fixed order — precipitation, NWS alerts, heat index, wind chill — each tagged with a kind (`precip`, `nws_alert`, `heat_index`, `wind_chill`) so the client can style them without parsing the message.

Only alerts whose validity window overlaps the hike window are included, with a missing bound treated as open-ended on that side. A winter storm warning starting nine hours after the hike ends is true and irrelevant, and an irrelevant alert on the screen teaches the reader to ignore the row.

The observation path returns no alerts at all. Active alerts are a statement about now; a past hike's alerts would require a historical query that does not fire.

## Version Output

| | v2 (frozen) | v3 |
|---|---|---|
| Temperature | `startTempF` / `endTempF` — first and last in-window periods | same |
| Conditions | `conditions` — first in-window period | `startConditions` / `endConditions` — first and last in-window periods |
| Precipitation | `probabilityPct`, `expected`, `startsAt`, `endsAt` | same |
| NWS alerts | window-overlapping only | same |
| Heat index / wind chill | reported when their gates are met | same |

A four-hour hike that starts at 62°F and ends at 81°F is badly described by one number, which is why both ends are reported. The same is true of the sky, and v2 does not yet say so: a hike that starts clear and ends in thunderstorms reads "Sunny", because conditions were never paired with the temperatures they describe. v3 pairs them.

## Precipitation Timing

`expected`, `startsAt`, and `endsAt` describe rain across the hike's **local calendar day**, not its window — the day of `start` in the record's own offset. Timestamps are emitted in that same offset and may fall before or after the hike.

The day boundary is local rather than UTC because the question being answered is a human one. A parent reading "rain starts at 2pm" is asking about their day, which begins at midnight where they live; a UTC day for a US-Central hike would run from 19:00 the previous evening, folding in the night before and cutting off the evening ahead.

An hour counts when its probability is at or above 50%. This is a different and much higher bar than the 1% that raises the precipitation alert, and deliberately so: the alert answers "is rain possible?" while the timing answers "when will it actually rain?". A timeline built from 10% hours would be noise.

The `probabilityPct` reported beside the timing is still the in-window maximum, so the number and the timing answer different questions about the same day.

Knowing rain arrives at 2pm lets a leader start early rather than cancel, which is why the span reaches beyond the window rather than being clipped to it.

## Decisions & Alternatives

| Decision | Chosen | Alternatives Considered | Rationale |
|---|---|---|---|
| Weather provider | NWS (api.weather.gov) | OpenWeatherMap; Tomorrow.io; Weather.gov gridpoint only | Free with no key, authoritative for US watches and warnings, and the alert feed the pack actually cares about. Cost: US-only and occasionally slow. |
| Past hikes | Station observations | Return no weather; keep serving the expired forecast | The hourly forecast is future-only; a screen that blanks the afternoon of the hike is worse than one reporting what happened. |
| Observation shape | Reshaped into the forecast period type | A separate observed-weather type and builders | Every builder downstream stays source-agnostic; the alternative would double the interpretation code. |
| Observed precipitation | Binary — any measured amount reads as 100%, none as 0% | Report the measured millimetres; scale to a pseudo-probability | For a hike that already happened the useful answer is whether it rained on them, not how likely it was. Certainty is the honest value. |
| Station selection | Nearest reporting station, up to three tried | First station only; query every station and merge | A listed station is not necessarily a reporting one, and stopping at the first turns one offline gauge into no weather at all. The cap bounds worst-case latency. |
| Coordinate precision in cache keys | Four decimals, matching the NWS request | Two decimals, to share entries across nearby points | A key coarser than the request it stands for can serve one point's data for another's query. Meeting points are fixed per location, so the sharing bought nothing. |
| Day boundary | The hike's local calendar day, for both precipitation timing and observation cache keys | UTC day; the hike window itself | Timing answers a human question about a human day. Using the same boundary for the cache key keeps one definition of "day" in the segment. |
| Cache backend | Workers Cache API | KV; Durable Object; no cache | Free and colo-local. KV would share across colos but adds a binding and eventual-consistency semantics for a forecast that is stale in ten minutes anyway. |
| Empty forecasts | Never cached; cached empties treated as a miss | Cache uniformly | An empty forecast is an upstream gap, not authoritative "no weather"; caching one suppresses weather for the whole TTL. |
| Empty observations | Cached for the full observation TTL | Never cached, as with forecasts | A completed hike whose nearby stations recorded nothing is a settled answer. Not caching it makes every view repeat the whole station walk for a result that cannot change. |
| NWS error bodies | Status ≥ 400 is a hard failure | Parse whatever came back | NWS returns JSON with its errors; parsing it yields an empty forecast indistinguishable from a real one. |
| Parse failures | Drop the record, keep the rest | Fail the forecast | One malformed hour should not cost the whole hike's weather. |
| Wind speed from a range | First number (the low end) | Midpoint; high end | Wind matters here only through wind chill, where the low end is the conservative reading. `[inferred]` |
| Unrecognized wind unit code | Assume km/h | Drop the reading; assume m/s | km/h is the NWS default, so an unrecognized code most likely is one. `[inferred]` |
| Precipitation alert threshold | 1% | 20%; 30%; NWS "chance" wording | Any nonzero chance is a gear decision for a pack of eight-year-olds. |
| Precipitation timing threshold | 50% | Reuse the 1% alert threshold | A timeline built from 10% hours is noise; timing answers a different question than the alert. |
| Timing span | Local calendar day | Hike window only; ± a few hours | Rain arriving an hour after the window still changes whether to start early; clipping to the window hides it. |
| NWS alert filtering | Window-overlapping only | Every alert active at the point | An alert beginning nine hours after the hike ends is true and irrelevant, and an irrelevant row teaches the reader to ignore the list. |
| Alerts for past hikes | None | Historical `/alerts` query | Active alerts are a *now* concept; a historical fetch is a third request for a screen nobody is reading in a parking lot. |
| Heat index / wind chill gating | Formula domains (≥80°F, ≤50°F and >3 mph) | Compute always and clamp | Reporting a number outside a formula's domain is inventing one; absent is honest. |
| Extreme reporting | Max heat index, min wind chill | Mean; value at hike start | A leader decides against the hardest moment of the hike. |

## Open Questions & Future Decisions

### Resolved

1. ✅ Observations for completed hikes, forecast otherwise.
2. ✅ Empty results are never cached in either direction.
3. ✅ Timing spans the local calendar day, not the hike window — and that same day boundary keys the observation cache.
4. ✅ Observed precipitation is binary; "did it rain" is the question a completed hike answers.
5. ✅ A non-reporting nearest station falls through to the next, up to three.
6. ✅ Conditions are paired with temperatures at both ends of the window, in v3.

### Known unhandled edge cases

These are understood, judged unlikely for this pack, and deliberately not handled. They are recorded so the behaviour is a known limitation rather than a surprise.

1. **Two completed hikes at one preserve on the same local day** collide on a single observation cache entry, because the key carries the date but not the window. The second hike is served the first's readings for the rest of the day.
2. **A hike spanning midnight** takes its precipitation timing from the local calendar day of `start` alone. Rain during the portion after midnight is excluded, so an overnight hike can report `expected: false` while it is raining on the group.

### Deferred

1. **Quantitative precipitation is absent from forecasts.** Only probability is reported. A gridpoint QPF fetch would provide an amount at the cost of a third NWS request.
2. **Observed rainfall is measured but discarded.** The observation feed carries millimetres in `precipitationLastHour`; the binary answer is derived from it and the number thrown away. Reporting it would need a field that means "amount observed" rather than "probability forecast".
3. **Cross-colo cache sharing.** The Cache API is per-colo, so each edge location fetches independently. KV would share, at the cost of a binding and eventual consistency.

## References

- [NWS API](https://www.weather.gov/documentation/services-web-api) — `/points`, `/gridpoints/.../forecast/hourly`, `/stations/.../observations`, `/alerts/active`.
- [NWS heat index (Rothfusz regression)](https://www.wpc.ncep.noaa.gov/html/heatindex_equation.html).
- [NWS wind chill formula](https://www.weather.gov/media/epz/wxcalc/windChill.pdf).
- `docs/intent/api-surface/api-surface-design.md` — the `WeatherSource` contract and how a weather failure degrades.
