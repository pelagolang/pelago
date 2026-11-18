Id: S-16
Title: Time library: moments, calendar, zones, formatting, timers
Column: Done
Order: -
Depends on: clocks on every backend (done, #276, #285)
Size: XL
Revisit when: -

# Why

The owner asked (2026-11-17) for `std.time` to be finished into a full system. Today it has a monotonic clock, the wall clock as a `duration`, `sleep` and `duration` arithmetic, with no test file of its own. Plan and audit: [docs/design/time.md](../../design/time.md).

# Plan

One PR per batch (design section 5):

1. **T1** `std.time` finished: `moment`, more `duration` units and arithmetic, checked and saturating forms, tests.
2. **T2** `std.calendar`: `date`, `time_of_day`, `date_time`, weekdays, ISO weeks, month arithmetic.
3. **T3** `std.zone`: offsets, TZif reader, local zone, `zoned`, ambiguous and missing local times as values.
4. **T4** `std.timefmt`: RFC 3339, ISO 8601, RFC 2822, patterns.
5. **T5** timers: `sleepUntil`, `deadline`, `ticker`, `stopwatch`, with `select`.
6. **T3b** `std.tzdata`: the bundled tz database, so `zone.named` works on Windows.

All six batches are merged (#310 to #313 and the earlier ones). Optional follow-ups: ISO week-date patterns, locale names, zone names in patterns, the interpreter's leak report counting the detached ticker task.

# Done means

- Every batch: `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` green; a `.pel` + `.out` test in `compiler/std/tests/`; Linux and Windows CI green.
- Values checked against Python (`datetime`, `zoneinfo`, `email.utils`) where not obvious.
- `docs/design/std.md`, `ROADMAP.md` and the website docs updated after each merge.
