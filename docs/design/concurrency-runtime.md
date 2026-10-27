# Concurrency, built out (plan)

Status: plan, written 2026-10-27 for card [A-12](../plans/cards/A-12-concurrency.md). Language rules are in
`concurrency.md` (CC1 to CC10) and do not change here. This page says how the rules become real: what the
library does, what the machine must give it, and in which order we build it.

## 1. Goal and constraints

- Everything in `concurrency.md` works for real: tasks that talk back and forth, bounded channels that block,
  `select` with a timeout, `scope` that joins, `lock`, `shared`, `atomic`, parallel loops, and then several cores.
- Stage-0 programs keep their meaning. No program has to change.
- **No libc.** The runtime period (owner decision, 2026-10-27) writes the OS layer once, as a whole. So all
  OS-level pieces of concurrency sit behind one small boundary (section 3) and nothing else in the compiler or the
  library knows what a syscall is. We do not add piecemeal OS code now.
- **A stop in any task stops the program** (CC1). No unwinding; `defer` and `deinit` do not run. With several
  threads the first stop wins: it sets a flag, flushes output once, and ends the process (`exit_group` / `ExitProcess`).
- No hidden runtime in the sense of a GC or a heap-allocated event loop the user cannot see: the scheduler is
  ordinary library code in `core.pel`, inlinable and readable.

## 2. The design in one picture

```
 language        spawn  scope  chan  select  lock  shared  atomic  par_for  coroutine
                   |      |      |      |      |      |       |       |        |
 library         task / scheduler / wait queues / timers / mutex / condvar   (core.pel, Pelago)
                   |
 machine layer   ctx_*   (stack switch, no OS)        atomic_* / fence   (IR ops)
                   |
 OS boundary     os_*    (clock, sleep, thread start, futex, stack memory, exit)
```

Two ideas carry the whole thing.

1. **Stackful tasks over a context switch.** A task has its own stack and the scheduler switches between stacks
   with a few instructions. A blocked `recv`, `join`, `lock` or `sleep` just parks the task and switches to another
   one; no state machine and no change to ordinary code. The same switch gives coroutines (generators) for free.
2. **The scheduler is a library with a worker count.** Stage 1 has one worker (one OS thread, cooperative): this
   already makes ping-pong, blocking channels, `select`, timeouts and deadlock detection real. Later the same
   scheduler runs N workers (one per CPU) that steal work. The code that uses tasks never learns the difference.

## 3. The OS boundary (what the runtime period must provide)

Eight functions. The compiler lowers them to `ext fn` intrinsics; each target implements them; the IR interpreter
implements them in Rust. Nothing else in concurrency touches the OS.

| Function | Meaning | Linux (no libc) | Windows | Interpreter |
|---|---|---|---|---|
| `os_clock_now() -> int` | monotonic nanoseconds | `clock_gettime` syscall (vDSO later) | `QueryPerformanceCounter` | `Instant` |
| `os_sleep_until(t)` / `os_park_until(word, t)` | stop this worker until a time or a wake | `futex` with timeout | `WaitOnAddress` | condvar |
| `os_wake(word, n)` | wake workers waiting on a word | `futex` wake | `WakeByAddress*` | condvar |
| `os_thread_start(fn, arg, stack) -> int` | start a worker | `clone` | `CreateThread` | `std::thread` |
| `os_stack_alloc(size) -> ptr` / `os_stack_free` | stack memory with a guard page, committed lazily | `mmap` + `mprotect` | `VirtualAlloc` | host `Vec` |
| `os_cpu_count() -> int` | worker count | `sched_getaffinity` | `GetSystemInfo` | 1 |
| `os_thread_id() -> int` | which worker am I (thread register) | `fs` base via `arch_prctl` | `gs` / TEB | fixed |
| `os_exit(code)` | end the whole process, all threads | `exit_group` | `ExitProcess` | return |

The machine layer (not OS) adds: `ctx_make(stack, fn, arg) -> ctx`, `ctx_switch(from: ptr, to: ctx)` (save callee-saved
registers and the stack pointer, load the other set: about ten instructions per target), and the IR atomics
`AtomicLoad`, `AtomicStore`, `AtomicRmw(add/and/or/xchg)`, `AtomicCas`, `Fence` (sequentially consistent first).
Stage 1 needs `ctx_*` plus `os_stack_alloc` and `os_exit`, which already exist in the backend as mmap and exit.

## 4. Library design

**Task.** A control block: state (new, ready, running, parked, done), stack, saved context, result slot, the
join waiter, detach flag, cancel flag. `spawn(f)` allocates a block and a stack, puts it on the run queue and
returns `task<T>`. `join!!()` parks the caller on the task's waiter slot until the task is done.
`main` returning ends the program (CC6 table) even with tasks still running; detached tasks are simply abandoned
(stage 0's "run them at the end" goes away: that was a stage-0 stand-in).

**Scheduler.** Run queue per worker plus a global overflow queue; `yield`, `park(waitq)`, `unpark(task)`. Idle
workers sleep in `os_park_until`. A timer heap (task wake times) feeds `sleep(ms(n))` and `select`'s `timeout`.
If every task is parked, no timer is pending and no I/O is outstanding, the program stops with
`deadlock: N tasks are waiting` and a list of what each waits on (stop message, not unwinding).

**Channels.** A queue guarded by the channel's lock, a waiter list for receivers and one for senders.
Bounded `send` parks when full (capacity is really enforced); `recv` parks when empty; closing wakes everyone.
Capacity 0 stays "unbounded", as in `concurrency.md`. A rendezvous channel is `chan<T>(rendezvous)` later if wanted.

**`select`.** Registers the task on every arm's waiter list with a shared "fired" word; the first arm to fire
wins with an atomic compare-and-swap; the task unregisters the others. `default` does not park; a `timeout` arm
adds a timer. With one worker the CAS is trivially uncontended.

**Locks.** `lock<T>`: an atomic word (0 free, 1 held, 2 held with waiters) plus a wait queue; spin briefly with
more than one worker, then park. `with` keeps its shape (closure gets `inout`). Also in the library, not the
language: `condvar`, `once`, `barrier`, `semaphore`, `rwlock` (read-mostly `shared` data), built from the same
word-and-queue pair.

**`shared<T>`** gets an atomic count (`AtomicRmw add`); **`atomic<T>`** is the IR atomics with a plain method
surface (`load`, `store`, `add`, `swap`, `compare_swap`).

**Allocator.** Stage 1 is single-worker, so unchanged. Before several workers: per-worker free lists (thread
register from `os_thread_id`) with a locked global fallback; frees from another worker go to the owner's list.

## 5. Structured concurrency

- `scope s { ... }` is real: the closing brace waits (parks) until every task started with `s.spawn` is done.
  The checker already enforces disjoint captures, so a scope may lend `let` views and disjoint `inout` to its tasks.
- Tasks started with `spawn` (not through a scope) are owned by their `task<T>` handle: dropping the handle
  detaches. `@must_use` warns.
- **Cancellation** is cooperative and explicit: `t.cancel()` and `s.cancel()` set a flag; `cancelled()` reads it;
  `select` has `case cancelled() { ... }`; blocking operations stay blocking (no hidden exceptions). `sleep` and
  `recv` with a deadline are the building blocks for time-limited work.
- A stop in a scoped task stops the program (CC1): there is nothing to unwind into, so no "sibling cancellation on
  error" policy is needed. Expected failures travel as `result` values, e.g. `join` of a `task<T ! E>`.
- `par_for` / `par_map` (CC8) are library functions over `scope` with `chunks_inout`; worker count from `os_cpu_count`.

## 6. Coroutines

Stackful coroutines are the same machinery without the scheduler: a context with its own stack, resumed and
suspended by the program. First as a library type (no new syntax), second as syntax if wanted:

```
const g = coroutine<int>(co ~> { for i of 0..3 { co.yield(i) } })    // library form
for x of g { print(x) }                                              // iterator: resume until done
```

- `coroutine<T>` implements the iterator interface, so `for x of g` works. Dropping an unfinished coroutine frees its stack
  and drops the live locals (needs the ledger to unwind a suspended stack: **open point, see Q3**).
- A syntax form (`fn evens() -> gen<int> { yield 1 }`) can be a thin lowering to this type; decide after the library form is used.
- `async` (S-04): with stackful tasks an `async fn` can be a task and `await` a `join`. That replaces the state-machine
  plan (cheaper to build, one extra stack per concurrent call, no "no view across await" rule needed). **Q2.**

## 7. Determinism and testing

- The interpreter implements `ctx_*` with host threads handing a baton (only one runs at a time), so scheduling is
  deterministic. `pelago run --seed N` picks the next ready task pseudo-randomly: 100 seeds per concurrency test is a
  cheap race and deadlock finder for the library. Native runs use the real scheduler; the differential test (interpreter
  vs native) compares the outcome of programs whose result does not depend on timing.
- New tests: ping-pong, worker pool of 8 draining a channel, bounded producer/consumer (send blocks when full),
  `lock` counter 8 x 10^5, `shared` count back to 1, `select` timeout that times out, `scope` joining at the brace,
  a detected deadlock, `main` returning with a running detached task, a stop inside a task.
- No heavy benchmarks. One small micro-benchmark per stage (spawn+join, channel round trip) for a number in the PR.

## 8. Stages (one PR each, in order)

| Stage | What | Needs |
|---|---|---|
| C0 | This plan, card A-12, S-02 and S-04 updated | nothing |
| C1 | Context switch: `ctx_*` IR ops, interpreter baton, x86-64 and AArch64 emission, `os_stack_alloc` | backend |
| C2 | Scheduler and blocking tasks in `core.pel`: `spawn`, `join`, `yield`, `sleep`, deadlock stop; `scope` joins; detached semantics | C1, `os_clock_now` |
| C3 | Blocking channels (bounded enforced), real `select` with timeout, `lock` with wait queue, `condvar`, `once`, `barrier`, `semaphore` | C2 |
| C4 | Coroutines (`coroutine<T>`, generator iteration) and cancellation (`cancel`, `cancelled`, `select` arm) | C2 |
| C5 | IR atomics, atomic `shared`, `atomic<T>`, memory-model note in `concurrency.md` | IR, interpreter, both backends |
| C6 | Several workers: `os_thread_start`, futex park, thread register, per-worker allocator, work stealing, `par_for`/`par_map` | C5, runtime period OS layer |
| C7 | `async` on tasks (if Q2 says yes), `select` over futures | C3, C4 |

C1 to C5 touch no OS service beyond what the backend already has (mmap, exit, a clock read), so they go ahead now.
C6 is the one that needs the OS layer in full; its `os_*` functions are written to this table and implemented in the
runtime period, with the interpreter version making it testable in the meantime.

## 9. Questions for the owner

1. **Stackful tasks (this plan) or stackless state machines** for tasks and coroutines? Recommend stackful: simplest,
   no new checker rules, one mechanism for threads, coroutines and async. Cost: a stack per task (reserve 256 KiB,
   guard page, committed on touch; a million tasks is not a goal).
2. **`async` on top of tasks** instead of state machines (S-04)? Recommend yes; keep the `async`/`await` keywords.
3. **Dropping a suspended coroutine or task**: run the pending drops by switching into it with a "cancelled" mode that
   makes it unwind through its drops (not a stop), or just free its stack and leak its locals? Recommend the first,
   scoped to coroutines and cancelled tasks only, as an explicit exception to "no unwinding". It never runs on a stop.
4. **Detached tasks at the end of `main`**: abandon (recommended; same as threads in C and Rust) or wait for them.
5. **Default stack size** per task: 256 KiB, overridable `spawn(f, stack: 1 MiB)`.

Until answered, stages C1 to C3 and C5 are independent of the answers; C4 and C7 wait for 2 and 3.
