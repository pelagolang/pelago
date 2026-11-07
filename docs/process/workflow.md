# Pelago development workflow

Principle: make it correct, make it measurable, then make it fast.

## Cycle (two weeks)

| Phase | Length | Allowed | Exit criterion |
|---|---|---|---|
| Build | ~5 days | Features, refactors, docs. No perf PRs. | Each feature has tests, an `examples/` program and a design-doc line. |
| Harden | ~3 days | Edge-case, fuzz and overflow-trap tests, bug fixes. No new features. | `cargo test` green on all CI targets; open bug list empty or triaged. |
| Baseline | ~1 day | One harness run on a pinned runner; commit `bench/baseline.json`. | Baseline file merged. |
| Optimise | ~4 days | Perf PRs only, against the baseline. | Each PR shows before/after rows from the harness; no row regresses beyond the noise threshold. |
| Retro | ~1 hour | Ten lines in `docs/NEXT.md`: shipped, broke, next. | Next cycle's cards chosen. |

Perf work never starts before Harden has finished.

## Rules

1. **Card cap.** At most 3 cards in "Doing" across the whole board (this replaces the per-track limit in `docs/plans/README.md`). A card enters "Doing" only with one owner thread and a one-line "done when".
2. **One owner per area.** Compiler front end, IR/optimiser, native backend, runtime/library, tooling. A thread touching another area's files asks the owner thread first.
3. **Small PRs.** One concern per PR; target under ~400 changed lines excluding goldens.
4. **Tests per area.** Split the large shared `tests.rs` files into one file per feature area so threads stop conflicting.
5. **Perf PRs** quote before/after numbers from `bench/harness/run.py`, state which parts are conventional and which use Pelago's own semantics, and keep exact overflow traps.
6. **Benchmark smoke job.** Triggered by a PR label (e.g. `bench`), runs a small subset against `bench/baseline.json`, and comments the diff with spread marks. The full harness stays manual (`workflow_dispatch`).
7. **Work queue.** Cards are ranked. Work is taken strictly from the top, so whatever finishes first is always the most valuable. Anything unfinished at the end of a window is parked in `docs/NEXT.md` with its state.
8. **Reports** to the owner are short and technical: result, numbers, what is next.

## Decisions

- **Smoke-job subset depends on the change.** The job picks programs by the paths a PR touches: lexer/parser changes run lexer and parse benchmarks; `ir/` and `lower/` changes run the compute-bound set; `native/` changes run everything plus binary size; runtime/heap changes run the allocation and channel programs. A PR can add programs with a second label. The full harness stays manual.
- **Regression threshold: 5%, or the measured spread if larger.** Revisit after the first three baselines show the real noise.

## Path-to-benchmark map (first version, lives in `bench/smoke-map.md`)

| PR touches | Smoke programs |
|---|---|
| `lexer.rs`, `parser/` | lexer, wordfreq, kostya_json |
| `ir/`, `lower/`, `typecheck/` | collatz, fib38, nbody, matmul, sieve, structloop |
| `native/` | everything above plus hello (binary size), bintrees, levenshtein |
| heap, arena, runtime, `core/` | bintrees, strsplit, strbuild, mapwork, structlist |
| channels, tasks | the concurrency programs (add when they exist in `bench/programs`) |
| anything else | collatz, lexer, bintrees (sanity trio) |

## Pinned runner

`blacksmith-4vcpu-ubuntu-2404` (bare metal, x86-64) for baselines and the smoke job, because it has the least noise of the runners already in `bench-harness.yml`. Fallback if the Blacksmith app is not installed on the org: `ubuntu-24.04`, with the threshold forced to at least 10% until the spread is measured. ARM numbers (`blacksmith-4vcpu-ubuntu-2404-arm`) are taken once per cycle at the baseline step, not per PR.

## Not built yet

`bench/baseline.json`, the PR-label smoke job and `bench/smoke-map.md` (the table above) are the next implementation step, as a separate piece of work.
