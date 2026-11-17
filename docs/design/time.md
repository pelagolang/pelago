# Time: clocks, calendar, zones, formatting and timers (draft 1)

Status: plan, 2026-11-17. T1 to T5 below are built in that order, one PR each. Requested by the owner: "finish out the
time lib into a full fledged system". Card: [S-16](../plans/cards/S-16-time.md).

## 1. What exists today (audit)

`std.time` is one file (`compiler/std/time.pel`, 127 lines) and **has no test of its own** (the clock is covered by
`compiler/tests/run_cli.rs`, `sleep_and_select_timeouts_use_the_clock`).

| Piece | State |
|---|---|
| `duration` (whole nanoseconds in an `int`, about 292 years either way) | built; `+ - neg == <`, `show`; plain `+ -` stop on overflow |
| `instant` (monotonic), `now`, `since`, `elapsed`, `plus` | built |
| `sinceEpoch()` (wall clock) | built, but returns a `duration`: the wall clock and the monotonic clock are not separate types for the wall side |
| Clocks on every backend | built: Linux (`clock_gettime`), Windows x86-64 and ARM64, macOS AArch64, interpreter |
| `sleep(d)`, `select` `case timeout(d)` | built (tasks sleep on the scheduler; no busy wait where a kernel sleep exists) |
| Calendar, zones, formatting, parsing | **none** |

Gaps that matter: no `duration * int`, `/ int`, no checked or saturating arithmetic, no `from float`, no dates,
no zones, no text form beyond `duration.show`.

## 2. Principles

1. **A bare `int` is never a time.** Every point and every length carries its type (`instant`, `moment`, `duration`,
   `date`, ...), so seconds and milliseconds cannot be mixed up. This is the idea `duration` already follows.
2. **Three timelines, three types, not mixable by accident.** `instant` (monotonic, only differences mean anything),
   `moment` (a point on the UTC timeline, from the wall clock: can be set back), and civil values (`date`,
   `time_of_day`, `date_time`: what a calendar and a wall clock *say*, with no zone). A `zoned` value ties a civil
   value to a zone and an offset. Rust has `Instant` and `SystemTime` and leaves the rest to `chrono`/`time`/`jiff`;
   we ship all of it in `std` because the owner asked for a full system.
3. **Overflow stops the program, as everywhere in Pelago.** Operators trap. Where a caller can reasonably run out of
   range (a date a million years ahead, a `duration` from user input) there are `checked...` (returns `option`) and
   `saturating...` forms. Nothing wraps.
4. **Errors are values** (`result`, `option`): parsing returns `time_error`, a missing zone returns `time_error`,
   a clock that cannot be read returns `io_error` as today.
5. **No libc, no global state.** Zone data is read from files through `std.fs` (section 6); the local zone is read
   from `TZ` and `/etc/localtime` through `std.env` and `std.fs`.
6. **Value semantics.** All of these are plain structs. Calendar arithmetic is by value and never mutates.

## 3. Conventional and ours

Conventional (taken from Rust or chrono on purpose; users expect it):

- `duration` arithmetic, `since`/`elapsed`, `sleep`, the monotonic/wall clock split, RFC 3339 as the default text form.
- Proleptic Gregorian calendar, ISO 8601 weekdays (Monday = 1), ISO week numbers.
- IANA tz database, TZif files, and the POSIX-TZ footer rule for dates after the last explicit transition.
- Days-from-civil and civil-from-days integer algorithms (Howard Hinnant's), exact and branch-light.

Our own:

- `moment` is `{ secs: int, nanos: int }` (seconds since the epoch plus a nanosecond part in `0..1000000000`, always
  normalised, so it orders and compares by fields). Years reach billions either way, where an `int` of nanoseconds
  (the `duration` shape) would stop at 1678 to 2262. `duration` stays nanoseconds in an `int` because a *length* of
  292 years is plenty and arithmetic on it is a single instruction.
- One `zoned` type, not a family (`DateTime<Tz>`, `OffsetDateTime`, `ZonedDateTime`): a `zoned` always carries both
  the zone and the resolved offset, so there is one type to learn.
- Ambiguous and missing local times (clocks going back or forward) are **never silently picked**: `zone.at(local)`
  returns a `local_result` enum (`one`, `twice`, `never`), and `resolve(...)` helpers state the policy in the name
  (`earlier`, `later`, `shiftForward`). Rust's `chrono` has `LocalResult` too; the named policies are ours.
- Formatting patterns are readable letters (`yyyy-MM-dd HH:mm:ss.SSS`), not `%`-codes (question 1).
- Leap seconds are not represented: like POSIX and every OS clock, a day has 86400 seconds. `23:59:60` is accepted
  on parse and folded into `23:59:59` (question 3).

## 4. Modules (flat names; the website groups them under "Time")

| Module | Contents |
|---|---|
| `std.time` | `duration`, `instant`, `moment`, `now()`, `wallNow()`, `sinceEpoch()`, `sleep`, `sleepUntil`, `deadline`, `ticker` |
| `std.calendar` | `weekday`, `month`, `date`, `time_of_day`, `date_time`, leap years, days in month, ordinal, ISO week, add days/months/years |
| `std.zone` | `offset`, `zone` (fixed, UTC, named, local), `zoned`, `local_result`, tz database reader |
| `std.timefmt` | `format`/`parse` with patterns, RFC 3339 (the default), ISO 8601 dates and durations, RFC 2822, HTTP date |

Splitting keeps the cost down: a program that only times a loop imports `std.time` and pays for nothing else.

## 5. Surface by batch

### T1: `std.time` finished (the clocks and `duration`)

- `duration`: `time.minutes(n)`, `hours(n)`, `days(n)` (a `days` is exactly 24 hours: calendar days live in `std.calendar`);
  `d * int`, `d / int`, `d % duration`, `d.div(other) -> int`; `checkedAdd/Sub/Mul` (option), `saturatingAdd/Sub/Mul`;
  `time.fromSeconds(f: float) -> duration ! time_error` (rejects NaN and out of range), `signum`, `isZero`,
  `isNegative`, `max`/`min`/`clamp`, `subsecNanos`, `time.zero`, `time.max`, `time.min`.
- `instant`: `checkedPlus`, `minus(d)`, `-` and `+` with a `duration`, comparisons, `show` (as an offset from the
  process's first reading is **not** shown: an `instant` has no printable meaning; `show` prints `instant(<nanos>)`
  for debugging only).
- `moment`: `time.wallNow() -> moment ! io_error`; `fromUnix(secs)`, `fromUnixMillis`, `unixSeconds()`, `unixMillis()`,
  `plus(duration)`, `minus(duration)`, `since(moment) -> duration` (checked: two moments centuries apart do not fit),
  comparisons. `sinceEpoch()` stays for compatibility and is `wallNow().sinceUnix()`.
- A test file `compiler/std/tests/time.pel` with expected output (there is none today), plus overflow stop tests.

### T2: `std.calendar`

- `date { year: int, month: month, day: int }` is built only through `calendar.date(y, m, d) -> date ! time_error`
  (validates) or `calendar.dateOrdinal(y, n)`, `fromDays(n)`; `days()` (days since 1970-01-01), `weekday()`,
  `ordinal()`, `isoWeek() -> (int, int)`, `isLeap()`, `daysInMonth()`, `plusDays(n)`, `plusMonths(n)` (clamps the day,
  Jan 31 + 1 month = Feb 28/29, documented), `plusYears(n)`, `until(other) -> int` days, comparisons, `show` (`2026-11-17`).
- `time_of_day { hour, minute, second, nanos }` (`00:00:00` to `23:59:59.999999999`), `plus(duration)` returning
  `(time_of_day, days carried)`, `show`.
- `date_time` (naive): `date` + `time_of_day`, `plus(duration)`, `since(date_time) -> duration`, `show` (RFC 3339 without offset).
- Exact for years -9999 to 9999 against Python's `datetime` in the test; the algorithms are exact over the whole `int` range of years that fits.

### T3: `std.zone`

- `offset` (whole seconds east of UTC, `-86399..86399`), `zone.utc`, `zone.fixed(offset)`, `zone.named("Europe/Paris") -> zone ! time_error`,
  `zone.local() -> zone ! time_error`.
- `moment.inZone(zone) -> zoned`, `zone.at(date_time) -> local_result`, `zoned.offset()`, `zoned.date_time()`,
  `zoned.moment()`, `zoned.name()` (abbreviation such as `CET`), `zoned.isDst()`, `plus(duration)` (exact time) and
  `plusDays/Months(n)` (calendar, keeps the wall-clock time and re-resolves in the zone).
- **tz database source**: a `TZif` reader (RFC 8536, versions 1 to 4, 64-bit data and the POSIX-TZ footer rule). Data is
  read from `$TZDIR`, `/usr/share/zoneinfo`, `/usr/lib/zoneinfo`, `/usr/share/lib/zoneinfo` (Linux, macOS). Windows has no
  zoneinfo directory: question 2 decides between a copy of the compiled database shipped with the `std` package (about
  400 KB, only loaded when named) and fixed offsets only on Windows.
- A parsed zone is a value that can be kept; `zone.named` is cached by the caller, not hidden in a global (principle 5).
- Tested against Python's `zoneinfo` for a generated list of (zone, moment) pairs around every transition for several
  zones, including a southern-hemisphere one, a half-hour one (Asia/Kolkata), a 45-minute one (Asia/Kathmandu) and
  one with no DST (Asia/Tokyo).

### T4: `std.timefmt`

- `timefmt.rfc3339(zoned) -> string`, `timefmt.parseRfc3339(text) -> zoned ! time_error` (fractional seconds of 1 to 9
  digits, `Z` and `+hh:mm`, lower-case `t`/`z` accepted on parse as RFC 3339 allows).
- `timefmt.parseDate("2026-11-17")`, `parseTime`, `parseDateTime`, ISO 8601 durations (`PT1H30M`, `P1DT2H`), ISO week dates (`2026-W47-2`),
  RFC 2822 (`Tue, 17 Nov 2026 09:47:23 +0000`) and the HTTP date form.
- `timefmt.format(value, pattern)` and `timefmt.parse(text, pattern)`, pattern letters (question 1):
  `yyyy` `yy` `MM` `M` `MMM` `MMMM` `dd` `d` `HH` `H` `hh` `h` `a` `mm` `ss` `S`..`SSSSSSSSS` `EEE` `EEEE` `xxx` `xx` `Z` `VV`
  (zone name) and `'quoted text'`. English names only in version 1 (no locale tables; `std` has no locale story yet).
- A bad pattern is a `time_error` at the first use, never a stop. The same patterns format and parse (round trip is a test).

### T5: sleep, timers and concurrency

- `time.sleepUntil(instant)`, `time.deadline(d) -> instant`, `instant.hasPassed()`.
- `time.ticker(period) -> chan<instant>`-style receiver that works inside `select` next to `case timeout(...)`, built on tasks and the scheduler's timer queue
  (no busy wait), with `stop`; missed ticks are dropped, never queued without bound.
- `time.timeout(d, task)` helper only if `select` with `timeout` proves clumsy in the examples; `select` stays the primitive.
- A `time.stopwatch` (start, lap, elapsed) for benchmarks and logs.
- All of it on every backend that has a clock today (Linux x86-64 and AArch64, Windows, macOS, interpreter).

## 6. Cost and speed

- `moment`, `date` and the zone lookup are plain integer code. Days-from-civil is about a dozen integer operations; no tables.
- A zone lookup is a binary search over the transition list (a `list<int>` of seconds), then the footer rule: O(log n).
- Nothing is allocated for a clock read, a `duration` or an `instant` (as today).
- `std.time` stays small (the imports are separate modules), so `import std.time` does not get slower to compile.

## 7. Open questions for the owner

Each has a recommendation; the build goes ahead on the recommendation and the answer can change it later.

1. **Format patterns.** Readable letters (`yyyy-MM-dd HH:mm:ss`, Java/Unicode style) or strftime `%Y-%m-%d %H:%M:%S`?
   Recommendation: letters; they read without a table and parse the same way. `%`-style can be added as a second
   spelling later if you want it.
2. **tz database on Windows.** Ship the compiled database inside the `std` package (about 400 KB, loaded only when a
   zone is named, always the same data on every OS) or read the system's zoneinfo where it exists and give Windows
   fixed offsets only? Recommendation: read the system's where it exists (smaller, follows OS updates) and ship the
   bundled copy as a fallback in a second step (T3b) so Windows and minimal containers work.
3. **Leap seconds.** Ignore them (a day is 86400 s, as POSIX and every OS clock) and accept `:60` on parse by folding it into `:59`.
   Recommendation: yes.
4. **`duration` range.** Keep whole nanoseconds in an `int` (about 292 years) with checked and saturating forms.
   Recommendation: yes; a wider `duration` costs every addition for a need only calendars have, and `moment` covers the long range.

## 8. Tests

A `.pel` program and its `.out` per batch in `compiler/std/tests/` (run by `library_test_suites`), with generator
scripts checked against Python (`datetime`, `zoneinfo`, `email.utils`) where the values are not obvious, as
`blake3_ref.py` and `gen_regex_corpus.py` do. Overflow traps get `run_cli.rs` stop-message tests. Clock tests check
what must hold (monotonic, plausible year), not values.
