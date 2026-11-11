# Handoff: Pelago concurrency (thread "Build out concurrency fully")

## Goal
Full concurrency for Pelago (tasks, channels, sync, scheduling, structured concurrency, coroutines), built in stages C0 to C7 in `pelagolang/pelago`. Plan: `docs/design/concurrency-runtime.md`. Card: `docs/plans/cards/A-12-concurrency.md`. Language rules: `docs/design/concurrency.md` (CC1: a stop in a task stops the whole program, no unwinding).

## Merged to main (all CI green on Linux, Windows, ARM, Windows-ARM, macOS)
- #128 plan (C0)
- C1 context switch (IR `CtxInit`/`CtxSwitch`, x86-64 and AArch64 routines, interpreter contexts)
- C2 scheduler (`compiler/core/tasks.pel`: spawn, scope, join, detach, deadlock detection)
- #150 C3a flat ring-buffer channels (`chan<T>(capacity)`, copy-on-send, `select`, `lock`, `shared`)
- #151 C3b clonable `lock` (`copy h`), `lock.withWhen`, `semaphore`, `barrier`

## Open PRs / unpushed work
None. Everything is on `main`. No local uncommitted work.

## Next steps, in order
1. Rest of C3: wait queues instead of polling (`rt_wait` in tasks.pel), `select` send arm on a bounded channel, timeouts (needs `os_clock_now`).
2. C5 IR atomics (Load/Store/Rmw/Cas/Fence in IR, interpreter, x86-64, AArch64), atomic `shared` count, `atomic<T>`. Best done together with C6.
3. C4 coroutines and cancellation: blocked on open questions 2 and 3.
4. C6 real worker threads: needs the runtime period (no libc, the 8-function `os_*` table in the plan).
5. C7 async on tasks: blocked on question 2.

## Build and test (PowerShell)
```
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test          # about 110 s
cargo test -- program_lock program_semaphore program_barrier    # filter by name
cargo run -- check compiler/core
```
AArch64 can only be tested in CI. Conventions: branch `claude/<type>/<name>`, Conventional Commits for commits and PR titles, fill the PR template and tick every box (N/A ticked and marked N/A), merge when CI is green. Do not run benchmarks unless asked.

## Known gaps
- Task stacks are `rt_alloc(262144)` with no guard page.
- Waits poll with an idle counter (deadlock = 1 full cycle with no progress), not wait queues.
- `select` send arm does not wait for room in a bounded channel.
- `main` waits for unfinished tasks at its end (open question 4).
- `shared(lock(0))` plus `get()` clones; use `copy` on a `lock` handle instead.
- No `condvar` or `once` type by decision: use `withWhen` and a lock around an `option`.

## Open questions for Dennis (plan section 9, unanswered)
1. Stackful tasks as the model. 2. Async on tasks. 3. Unwinding for dropped coroutines and cancelled tasks. 4. Abandon detached tasks at end of `main`. 5. 256 KiB default task stack.
