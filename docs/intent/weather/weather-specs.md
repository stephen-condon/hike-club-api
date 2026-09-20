# Weather — Specs

Prefix: `WX`. Design: [`weather-design.md`](weather-design.md).

## Source selection and fetching

- [x] **WX-SRC-001**: While a hike's end time is in the past, the system shall source its weather from National Weather Service station observations rather than the hourly forecast.
- [x] **WX-SRC-002**: While a hike's end time is at or after the current time, the system shall source its weather from the National Weather Service hourly forecast together with the alerts active at the meeting point.
- [x] **WX-SRC-003**: The system shall send a `User-Agent` header identifying the application and a contact address on every National Weather Service request.
- [x] **WX-SRC-004**: The system shall format coordinates to four decimal places in National Weather Service request URLs.
- [x] **WX-SRC-005**: If a National Weather Service response carries an HTTP status of 400 or above, then the system shall treat that request as failed rather than parsing its body as data.
- [x] **WX-SRC-006**: When fetching a forecast, the system shall resolve the hourly forecast URL from the `forecastHourly` property of the `/points/{lat},{lon}` response.
- [x] **WX-SRC-007**: When fetching observations, the system shall resolve the station list URL from the `observationStations` property of the `/points/{lat},{lon}` response.
- [ ] **WX-SRC-008**: When fetching observations, the system shall try stations in the proximity order the National Weather Service returns them, advancing to the next station when one yields no readings for the hike window, trying at most three stations.
- [x] **WX-SRC-009**: The system shall format observation query timestamps with a trailing `Z` rather than a numeric UTC offset.
- [x] **WX-SRC-010**: The system shall reshape station observations into the same period representation used for forecast periods, so that no builder depends on which source produced a period.
- [x] **WX-SRC-011**: If any National Weather Service request fails, then the system shall report a weather failure rather than a partial result.

## Caching

- [x] **WX-CACHE-001**: The system shall cache National Weather Service results in the Workers Cache API and serve a cached result in preference to fetching.
- [ ] **WX-CACHE-002**: The system shall key a cached forecast by the meeting point's coordinates at four decimal places.
- [ ] **WX-CACHE-003**: The system shall key cached observations by the meeting point's coordinates at four decimal places together with the hike's local calendar day.
- [x] **WX-CACHE-004**: The system shall cache forecasts for 600 seconds and observations for 86400 seconds.
- [x] **WX-CACHE-005**: If a freshly fetched weather result contains no periods, then the system shall return it without writing it to the cache.
- [x] **WX-CACHE-006**: If a cached weather entry contains no periods, then the system shall treat it as a cache miss and fetch again.
- [x] **WX-CACHE-007**: The system shall cache the full hourly forecast for a point rather than the periods filtered to one hike's window.

## Parsing

- [x] **WX-PARSE-001**: The system shall drop any forecast period lacking a parseable start time, end time, or temperature, and retain the remaining periods.
- [x] **WX-PARSE-002**: The system shall read a forecast period's wind speed from its prose value by taking the first number, so that a stated range is read as its low end.
- [x] **WX-PARSE-003**: The system shall default a forecast period's precipitation probability to zero and its condition phrase to empty when either is absent.
- [x] **WX-PARSE-004**: The system shall drop any station observation lacking a parseable timestamp or temperature, and retain the remaining observations.
- [x] **WX-PARSE-005**: The system shall convert station observation temperatures from Celsius to Fahrenheit.
- [x] **WX-PARSE-006**: The system shall convert station observation wind speeds to miles per hour, reading the unit code as metres per second when it says so and as kilometres per hour for any other or missing code.
- [x] **WX-PARSE-007**: The system shall record a station observation's precipitation as 100 percent when its measured last-hour precipitation is above zero and as 0 percent otherwise, answering whether rain fell rather than how likely rain was.
- [x] **WX-PARSE-008**: The system shall give each station observation a one-hour span beginning at its timestamp, so that readings bracketing a hike window are treated as covering it.
- [x] **WX-PARSE-009**: The system shall sort station observations into ascending time order before any builder reads them.
- [x] **WX-PARSE-010**: The system shall read an alert's validity bounds from `onset` and `ends`, falling back to `effective` and `expires`, and shall treat a missing bound as open-ended.
- [x] **WX-PARSE-011**: The system shall drop any alert lacking an event name.

## Window filtering

- [x] **WX-WIN-001**: The system shall treat a weather period as within a hike window when the period starts before the hike ends and ends after the hike starts.
- [x] **WX-WIN-002**: If no weather period falls within the hike window, then the system shall produce no weather block for that hike.

## Derived signals

- [x] **WX-ALERT-001**: The system shall report a hike's precipitation probability as the maximum across the periods within its window.
- [x] **WX-ALERT-002**: The system shall compute heat index by the National Weather Service Rothfusz regression over in-window periods at or above 80°F that carry a relative humidity, and shall report the highest value found.
- [x] **WX-ALERT-003**: The system shall compute wind chill by the National Weather Service formula over in-window periods at or below 50°F whose wind speed exceeds 3 mph, and shall report the lowest value found.
- [x] **WX-ALERT-004**: While no in-window period satisfies the temperature and wind gates of a derived signal (heat index or wind chill), the system shall report that signal as absent rather than as zero, and shall raise no alert for it.

## Alerts

- [x] **WX-ALERT-005**: When a hike's in-window maximum precipitation probability is at or above 1 percent, the system shall raise a `precip` alert naming that probability.
- [x] **WX-ALERT-006**: When a hike's reported heat index exceeds 85°F, the system shall raise a `heat_index` alert naming that value.
- [x] **WX-ALERT-007**: When a hike's reported wind chill is below 32°F, the system shall raise a `wind_chill` alert naming that value.
- [x] **WX-ALERT-008**: The system shall emit a hike's alerts in the order precipitation, National Weather Service alerts, heat index, wind chill.
- [x] **WX-ALERT-009**: The system shall tag each alert with a kind of `precip`, `nws_alert`, `heat_index`, or `wind_chill`.
- [x] **WX-ALERT-010**: Under API version 1 the system shall include every National Weather Service alert active at the meeting point, without filtering by time.
- [x] **WX-ALERT-011**: Under API versions 2 and 3 the system shall include only those National Weather Service alerts whose validity window overlaps the hike window, treating a missing bound as open-ended on that side.
- [x] **WX-ALERT-012**: While a hike's weather is sourced from station observations, the system shall report no National Weather Service alerts.

## Version output

- [x] **WX-OUT-001**: Under API version 1 the system shall report temperature as the first in-window period's temperature and conditions as that period's condition phrase.
- [x] **WX-OUT-002**: Under API version 1 the system shall report precipitation amount as 0.0 inches.
- [x] **WX-OUT-003**: Under API versions 2 and 3 the system shall report `startTempF` from the first in-window period and `endTempF` from the last.
- [x] **WX-OUT-004**: Under API version 2 the system shall report conditions as the first in-window period's condition phrase.
- [ ] **WX-OUT-005**: Under API version 3 the system shall report `startConditions` from the first in-window period's condition phrase and `endConditions` from the last in-window period's condition phrase.
- [x] **WX-OUT-006**: Under API versions 2 and 3 the system shall report precipitation `expected` as true when any hour on the hike's local calendar day carries a precipitation probability at or above 50 percent, and false otherwise.
- [x] **WX-OUT-007**: Under API versions 2 and 3 the system shall report `startsAt` as the earliest start and `endsAt` as the latest end among the hike's local-calendar-day hours at or above 50 percent precipitation probability, expressed in the hike's own UTC offset, and shall omit both when no such hour exists.
- [x] **WX-OUT-008**: Under API versions 2 and 3 the system shall report precipitation `probabilityPct` as the in-window maximum, independent of the 50 percent threshold that governs timing.
- [x] **WX-OUT-009**: The system shall determine a hike's local calendar day from the UTC offset written in that hike record's `start`.
