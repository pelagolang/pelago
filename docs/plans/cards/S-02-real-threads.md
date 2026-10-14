Id: S-02
Title: Real threads for tasks, channels and locks
Column: Shelf
Revisit when: the machine-code backend can emit atomic instructions and start a thread (Phase 3), or when someone needs a benchmark that must use several cores.
Size: L
Touches: backend runtime layer, `core.lang` (task, channel, shared, lock), the interpreter (optional)

# Why tasks do not run in parallel today

Parallel threads need hardware support that only the backend can give: starting a thread, atomic reads and writes, memory ordering, a lock. The IR interpreter is one Rust thread, so it cannot run two tasks at once
no matter how the library is written. Stage 0 therefore runs tasks one after another, which keeps the program's *meaning* (what `spawn`, `join`, channels, `select` do) and tests it, but gives no speed-up.
The rules that make real threads safe already exist in the checker (`sink` or copy capture, no shared `inout`, `shared` read-only, `lock` for mutation), so **no program has to change** when threads become real.

# What the work is

1. **IR operations** (add to `ir.md` as decision IR25): `AtomicLoad`, `AtomicStore`, `AtomicAdd`, `AtomicCas` with an ordering (start with sequentially consistent only), `Fence`, and `ThreadSpawn(fn, env)` / `ThreadJoin(handle)`. The verifier checks operand types; the interpreter implements them with a deterministic scheduler (below).
2. **Deterministic interpreter scheduling (before the backend):** implement threads as *green threads inside the interpreter* with explicit yield points (at atomic operations, locks, `recv`, `join`, and every N steps), driven by a seeded scheduler. Gives reproducible runs and, with different seeds, a cheap **race and deadlock finder** for the library. This replaces today's "run when waited for" with a real interleaving.
3. **Library:** `shared<T>`: atomic reference count (`AtomicAdd`); `lock<T>`: a real mutex (futex-style: an atomic word, spin briefly, then park); channels: a queue protected by the lock plus waiting lists; `task`: a real thread handle; `scope`: join all tasks at the closing brace (right now each runs where spawned); `select`: wait on several channels at once.
4. **Backend runtime layer (Phase 3):** thread start through the OS (direct system calls on Linux: `clone`, `futex`; no libc, as the project aims), thread-local storage for the task's stack, stack size policy, a clean exit when `main` returns with tasks still running (detached tasks are joined or killed: **decision needed**).
5. **Allocator:** the heap must be safe to call from several threads: per-thread caches with a shared fallback, or a lock at first.
6. **Memory model note:** document that data-race freedom is by construction (the checker), so the only ordering needed is at the library's synchronisation points. State the guarantee in `concurrency.md`.

# Tests

- The existing task, channel, `shared`, `lock`, `select` tests keep passing unchanged (the real behaviour is allowed to differ only in timing).
- New: ping-pong between two tasks (impossible today), a worker pool with 8 workers draining a channel, a producer/consumer with a bounded channel (capacity actually enforced: `send` blocks when full), a `lock` counter incremented 10^5 times from 8 tasks equals 8·10^5, `shared` count returns to 1 after all handles drop, a `select` with timeout that really times out, `scope` joining on exit, a deadlock that is *detected* in the green-thread interpreter and reported.
- Run each concurrency test under 100 seeds in the interpreter.

# Not now because

Everything above except step 2 needs the backend. Step 2 could be done earlier, but until a program really needs ping-pong it adds scheduler complexity to a stage-0 tool.
