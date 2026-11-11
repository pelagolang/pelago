Id: A-12
Title: Concurrency built out: tasks, channels, sync, scheduling, structured concurrency, coroutines
Column: Doing
Order: 12
Depends on: A-09 (native backend); C6 depends on the runtime period (own OS layer, no libc)
Size: XL
Plan: [docs/design/concurrency-runtime.md](../../design/concurrency-runtime.md)

# What

Make every rule of `concurrency.md` real, in the stages C0 to C7 of the plan: stackful tasks over a context switch,
a library scheduler, blocking channels and `select`, locks and friends, coroutines, IR atomics, then several
workers. Replaces the "run tasks one after another" stand-in (IR18). Supersedes S-02 (threads) and, if Q2 is
accepted, S-04 (async suspension).

# Done means

- Each stage's tests in the plan section 7 pass in the interpreter (100 seeds) and natively, on all targets.
- `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` are green.
- IR18 and `concurrency.md` are updated as each stage lands.

# Progress

- C0 plan: done (#128).
- C1 context switch (`CtxInit`, `CtxSwitch`, interpreter baton, x86-64 and AArch64): done (#131).
- C2 scheduler and blocking tasks in `tasks.pel`: done (#147).
- C3a flat ring-buffer channels with enforced capacity, `lock` that waits: merged.
- C3b (merged) clonable `lock` (`copy h` shares it across tasks), `lock.withWhen(ready, f)` instead of a `condvar` (no separate `once`), `semaphore`, `barrier`. `select` send arms now wait for room in a bounded channel. Left in C3: a `select` timeout (needs the clock).
- C3c: waits park instead of polling (`rtPark`, an event counter and one parked word per task); deadlock is detected exactly when every task is parked. Per-object wait queues wait for several workers (C6).
- C4 (merged): `coroutine<T>` with `emit`/`next`/`iter`/`cancelled`, `task.cancel()` and `cancelled()`. Owner decisions on questions 1 to 5 recorded in the plan.
- C4b (in review): `scope.cancel()`, `select` `case cancelled()`, scopes wait only for their own tasks, detached tasks abandoned at the end of `main`. Guard pages on task and coroutine stacks: in review. Left: C7 `async`.
