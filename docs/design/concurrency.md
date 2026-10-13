# Concurrency (draft 1)

Goal: threads that cannot have data races, with no new borrow-checker rules. The access modes we
already have (`let` / `inout` / `sink`) and second-class references do the work. Async stays deferred (G5).

## 1. The one rule

> **Nothing is shared and changeable at the same time.** A value is either owned by one thread, or
> shared read-only, or behind a lock.

Value semantics already gives the first two for free. A data race needs two threads touching one memory
location with at least one writing; we make the compiler refuse to write that program.

What crosses a thread boundary:

| Way | What happens | Cost |
|---|---|---|
| `sink` (move) | The value goes to the other thread; the sender can no longer use it | none, a pointer move |
| copy | The other thread gets its own copy (`copy xs` for owning types) | a copy |
| read-only view in a **scope** (section 3) | Several threads read the same data; nobody writes | none |
| `shared<T>` (section 5) | Reference-counted, read-only | one atomic count |
| `lock<T>`, `atomic<T>` (section 5) | Changeable from several threads, serialised | the lock |

A closure given to a thread may capture only by `sink` or by copy (or by scoped read-only view). Capturing
`inout` is rejected: that is exactly the shared-and-changeable case.

**Sendable types are worked out by the compiler.** A type can cross a thread if everything inside it can.
Plain data, owning lists, maps, `obj` values (singly owned), `shared<T>` and channels can. Raw pointers
and views cannot. There is no `Send` / `Sync` marker to write; an error says which field blocks it.

## 2. Spawning

```
const h = spawn(sink data ~> crunch(data))     // runs on a new thread; returns task<int>
// ... do other work ...
const total = h.join!!()                       // waits; consumes the handle; gives the int
```

- `spawn(f)` starts `f` on a new OS thread and returns a `task<T>`.
- `join!!()` consumes the task and returns the result. If the task stopped (a bug, section 6 of
  `errors.md`) the **whole program stops**: we have no unwinding, so there is no half-dead program to
  recover into. Expected failures are returned as a `result` like in any function.
- Dropping a `task` without `join` detaches the thread. Use `@must_use`
  on `spawn` so forgetting is a warning.
- `spawn` is a normal prelude function, not syntax. (`spawn` stays a soft keyword so it can also be a name.)

## 3. Scopes: threads that borrow

`spawn` needs `sink` or copies because the new thread may outlive the caller. A **scope** guarantees every
thread inside ends before the scope does, so threads may read the caller's data in place:

```
fn total(xs: [int]) -> int {
    var left = 0
    var right = 0
    const mid = xs.len() / 2
    scope s {
        s.spawn([inout left] ~> left = sum(xs[..mid]))     // inout of a *different* variable
        s.spawn([inout right] ~> right = sum(xs[mid..]))
    }                                                      // both joined here
    return left + right
}
```

Rules, all checkable locally:

- Inside a scope, a thread may capture `let` views of outer data (nobody can change it while the scope runs,
  because the scope holds the outer name read-only) and may take **`inout` of a variable no other thread in
  the scope touches**. The compiler checks the captures are disjoint, the same exclusivity check as for one
  call with two `inout` arguments.
- Views cannot escape the scope (they are second-class already).
- Splitting one list for several writers is a library function that hands out non-overlapping pieces:

```
scope s {
    for inout chunk of data.chunks_inout(1000) {
        s.spawn([inout chunk] ~> process(inout chunk))
    }
}
```

This is the structured-concurrency model of Rust's `thread::scope`, and the safe core of "parallel for".

## 4. Channels

```
const (tx, rx) = chan<int>()          // sender and receiver ends

spawn(sink tx ~> {
    for i of 0..10 { tx.send(i) }     // send takes its value with sink: it is *moved*, never shared
})                                    // tx is dropped here: the channel closes for senders

for x of rx { print("got #x") }       // loop ends when every sender is gone
```

- `chan<T>()` gives a `(sender<T>, receiver<T>)` pair. Unbounded by default; `chan<T>(8)` is bounded and
  `send` waits when full.
- `tx.send(v)` moves `v` in. If the receiver is gone it returns `result<void, closed>` (an expected failure,
  not a bug); `tx.send(v)?` propagates it.
- `rx.recv()` waits and returns `option<T>`: `none` means the channel is empty **and** every sender is gone.
  `rx.try_recv()` never waits.
- `for x of rx` is `recv` in a loop.
- Many senders: `tx.copy()` (explicit, as for every owning type). One receiver; a receiver is moved, not copied.
  (A many-receiver work queue is a library type later.)
- Because a channel only moves values, **sending needs no lock-free trick from the user**, and the sent value
  cannot be touched by the sender afterwards (the compiler rejects it, as for any `sink`).

### `select`

```
select {
    case v = a.recv()   { print("a: #v") }
    case v = b.recv()   { print("b: #v") }
    case tx.send(next)  { sent += 1 }
    case timeout(50ms)  { print("slow") }
    default             { }                 // optional: do not wait
}
```

`select` is a statement like `switch`: arms end at the next `case`, no fallthrough, and `break` leaves the
enclosing loop. It waits for the first ready arm. A `recv` arm binds an `option` (`none` means that channel closed); see CC5.

## 5. Shared state, when you really need it

Most programs only need sections 2 to 4. For the rest, three library types. All are `obj`-like owners
with value semantics at the edges:

```
const config = shared(load_config())            // shared<config>: read-only, counted, cheap to copy
spawn(sink config.copy() ~> use(config))

const hits = lock(0)                            // lock<int>
hits.with!(n ~> n += 1)                         // the only way in: a closure gets inout access

const done = atomic(false)                      // atomic<bool>
done.store(true)
if done.load() { ... }
```

- `shared<T>`: read-only. To change it you need `lock<T>` inside, or copy it out.
- `lock<T>`: you cannot get a long-lived reference to the inside. `with` gives the closure a temporary `inout`
  and releases the lock when it returns. That removes forgotten unlocks, and "guards that outlive the lock"
  cannot be written because references are second-class.
- `atomic<T>` for integers, `bool` and pointers; methods `load`, `store`, `add`, `swap`, `compare_swap`, each
  with an ordering argument defaulting to sequentially consistent.
- Deadlocks are not prevented (nobody can, cheaply). A debug build may detect lock-order cycles later.

## 6. Interaction with the rest

| Topic | Rule |
|---|---|
| Stops | A stop in any thread stops the program, after running `@on_stop` once. No unwinding. |
| `deinit` | Runs on the thread that drops the value. Moving a value to another thread moves its cleanup too. |
| `main` returning | Returning from `main` ends the program even if detached tasks run. Join what you need. |
| `become` | Tail calls are per thread; no interaction. |
| Closures | Closures are values; one that captured only sendable things is sendable. |
| `@derive` | No effect; sendability is automatic. |
| Allocation | The allocator interface (Phase 4) must be callable from any thread; a per-thread cache is an optimisation. |
| Async | Still deferred. When it comes it can build on tasks and channels (an executor is a library). |
| Atomic keyword | `atomic` and `volatile` stay soft keywords, now used as `atomic<T>` and as a field marker for hardware memory. |

## 7. What we are not doing

- No shared mutable globals. A global `var` is not allowed; globals are `const`, or an explicit `lock<T>` / `atomic<T>`.
- No thread-local magic in v1.
- No async runtime, no green threads, no hidden scheduler (this fits "no runtime").
- No `volatile` rules here; that belongs with the hardware page (Phase 3).

## Questions

**CC1. Plain threads first.** Is `spawn` plus `task<T>` with `join!!()` right, with a stop in a task stopping the
program? (Alternative: `join` returns `result<T, stopped>`; that needs unwinding, so I advise against it.)

**CC2. Scopes.** Keep `scope s { ... }` as the borrowing form, with disjoint `inout` captures checked by the
compiler? The word `scope` would become a soft keyword. (Alternative: only `spawn` with `sink` and copies. Simpler,
but then parallel loops over borrowed data need copies.)

**CC3. Channel ends.** `const (tx, rx) = chan<T>()` with two ends, `tx.copy()` for more senders, `rx` not copyable.
Good, or one `chan<T>` value usable from both sides (Go style)?

**CC4. `send` after the receiver is gone.** Return `result<void, closed>` (my choice) or stop the program?

**CC5. `select`.** Keep it as a statement with `case` arms like `switch`? And when a channel closes, should the arm
get `none` (and you test it), or should closed channels simply never fire (Go sends a zero value, which is a trap)?
My advice: the arm binds the `option`, so `case v = a.recv()` has `v: option<T>`.

**CC6. Shared state types.** Names `shared<T>`, `lock<T>` and `atomic<T>`, with `lock` giving access only through
`with` (a closure gets a temporary `inout`)? (`mutex` is the usual word; `lock` is shorter and reads as a noun and a verb.)

**CC7. Global variables.** Forbid global `var` entirely (my advice) so a shared mutable global is impossible?

**CC8. Parallel loop.** Add a library helper like `par_for(inout xs, x ~> ...)` over `scope` now, or leave it for the
standard library phase?

**CC9. Time words.** `50ms` as a literal suffix for durations (`timeout(50ms)`), or `ms(50)` as a plain function?
