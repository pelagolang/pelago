# The runtime (draft 1)

What a Pelago program needs from the machine, how much of it exists, and the plan for the rest. Written
2026-11-06 from the audit in the project thread "Runtime design and namespaces". Language rules live in the other
design pages; this page is about what runs underneath them. Threads and the scheduler are in
[`concurrency-runtime.md`](concurrency-runtime.md) and are not repeated here.

**Decided by the owner (2026-10-27):** no `libc` whatsoever; a panic stops the program immediately (no unwinding,
`defer` and `deinit` do not run, see `errors.md`); the runtime is built in one dedicated period, as a whole, not in
small pieces.

## 1. Where we are

| Part | State |
|---|---|
| Start, exit, `Stop(msg)`, output buffer, allocator, stack guard, traps | built, x86-64 and AArch64, Linux, Windows, macOS (`backend.md` section 3) |
| Lists, strings, maps, sets, sort, objects, closures | built, written in Pelago in `compiler/core/core.pel` over the `rt_*` intrinsics |
| Tasks, channels, `shared`, `lock`, `select`, `async` | semantics built, run one after another (`concurrency-runtime.md`) |
| stdin, files, stderr, directories | missing (only `print` to stdout) |
| `main(args)`, environment, exit status | `fn main(args: [string])` passes the checker and stops at run time (`expected an address`); no `exit(code)` |
| Time, sleep, random | missing |
| Calling code outside Pelago (FFI) | not designed; `ext fn` only means "the backend provides it" |
| `std` package | does not exist; `import std.io` is "no module or package `std`" |

## 2. The OS boundary

One small layer, `os_*`, is the only place that knows what a system call is. Everything above it (the library,
the IR, the checker) sees plain functions. Per target it is raw system calls on Linux, the BSD calls on macOS (as the
runtime does today) and the few `kernel32` imports on Windows. No `libc`, no `libSystem` beyond what is already
needed to start, no dynamic linker for our own code.

First set (the whole of version 1):

| Area | Functions (names are placeholders) |
|---|---|
| Process | `os_args`, `os_env`, `os_exit(code)` |
| Streams | `os_read(fd, buf, n)`, `os_write(fd, buf, n)`; fds 0, 1, 2 |
| Files | `os_open`, `os_close`, `os_seek`, `os_stat`, `os_unlink`, `os_rename`, `os_mkdir`, `os_readdir` |
| Time | `os_clock_mono`, `os_clock_wall`, `os_sleep` |
| Memory | `os_map`, `os_unmap`, `os_protect` (already in the allocator) |
| Randomness | `os_random_bytes` (`getrandom`, `BCryptGenRandom` is not allowed without a DLL, so Windows uses `RtlGenRandom` from `advapi32` or the system-call table: **question R3**) |
| Threads | `os_thread_start`, `os_futex_wait/wake` (see `concurrency-runtime.md` section 3) |

Errors come back as one small `os_error` code, turned into the library's `io_error` (`errors.md`) in Pelago.

## 3. The standard library split

- **`core`** stays the always-in-scope prelude: the interfaces, `option`/`result`, lists, strings, maps, `print`,
  `assert`, `panic`. Nothing in it touches the OS except `print`.
- **`std`** is a bundled package next to `core`, reached with ordinary imports: `std.io` (stdin, stdout, stderr,
  `read_file`, `write_file`), `std.fs`, `std.env` (args, variables), `std.time`, `std.random`, `std.os` (exit,
  process id). `import std.io` then works with the module rules in `modules.md` unchanged.
- Moving a name out of `core` into `std` is a breaking change, so decide before 1.0 (**question R1**).

## 4. Start-up, `main` and exit

- Order: map the stack, set up the allocator and the output buffer, read args and environment from the initial
  stack (Linux) or the command line (Windows), build the `[string]` for `main(args)`, run global initialisers,
  run `main`, flush output, `exit(status)`.
- `main` may return nothing, a status `int`, or `void ! E`; an `err` prints `error: <message>` to standard error and
  exits 1 (`functions.md` F6 already allows `result`).
- `exit(code)` flushes the output buffer and ends the process; it does not run `defer` or `deinit`.
- `print` stays buffered (64 KiB); standard error is unbuffered and flushes standard output first, so the order is
  readable.

## 5. Panics and stops

One mechanism: `Stop(msg)`. It flushes standard output, writes `stopped: <msg>` to standard error and exits with
status 1. The message should gain a source location (`file:line:col`) once the IR carries spans (**question R4**).
A stop in a task stops the whole program (CC1); the first stop wins. Nothing unwinds. This is one rule for
assertion failures, overflow, division by zero, bounds, `unreachable`, out of memory and stack overflow.

## 6. Memory

The allocator contract (`backend.md` section 3) is kept: size-class free lists, large blocks mapped directly,
zeroed blocks, leak counter in test builds. Open for this period: per-thread caches (needed by real threads), an
arena type for programs that want to free everything at once, and an `alloc` failure policy (today: stop).
Custom allocators in the library API are not in version 1.

## 7. Calling code outside Pelago (FFI)

Because there is no `libc`, "calling C" means two different things:

1. **Our own OS layer**: `ext fn` in `core`/`std` bound to the `os_*` routines. Done as part of section 2.
2. **Foreign libraries** (`ext "C" fn puts(...)`): needs a platform symbol resolver. On Windows the PE import table
   already does it. On Linux a static binary has no dynamic linker, so this needs either a dynamic ELF with an
   interpreter, or `dlopen` written by hand. It is large, and not needed for a language that does its own I/O.
   **Recommendation: not in version 1.** Keep the calling convention documented so it can be added later
   (**question R2**).

## 8. Work packages for the runtime period

In dependency order; each can be one PR.

1. This page and the `ROADMAP.md` rows, after the owner answers the questions.
2. `os_*` for Linux x86-64: args, `exit`, `read`/`write`, files, time, random; interpreter versions beside them.
3. `main(args)` fixed, `exit`, `eprint`, status from `main`; tests that run natively and in the interpreter.
4. `std` package (`io`, `fs`, `env`, `time`, `random`) and the `import std.x` resolution.
5. The same `os_*` on AArch64 Linux, Windows and macOS.
6. Allocator for several threads, then `concurrency-runtime.md` stages.
7. Spans in `Stop` messages.

## Related work already on the board

- **Threads and OS table:** `concurrency-runtime.md` section 3 already fixes the thread-side `os_*` functions
  (`os_clock_now`, `os_sleep_until`/`os_park_until`, `os_wake`, `os_thread_start`, `os_stack_alloc`/`os_stack_free`,
  `os_cpu_count`, `os_thread_id`, `os_exit`). The first set in section 2 above must use those names and signatures;
  this page adds the I/O, files, args, environment and random functions around them.
- **Heap:** `heap.md` (H0 to H6) replaces the allocator with reserved address ranges and spans. Its open questions
  (reservation size, zeroing rule, where the allocator is written, huge pages) belong to this period too.
  `arena.md` (A-13) is merged and its own questions are open.
- **`std`:** an unmerged branch `claude/feat/std-libs` (`std.big`, `std.json`, `std.regex`, `docs/design/std.md`)
  already makes a `std` package. Section 3 must be reconciled with it before work package 4: `std.io`, `std.fs`,
  `std.env`, `std.time` and `std.random` are the OS-facing modules, the branch has the pure-library ones.
- **Open questions of the concurrency thread** (stackful tasks as the model, async on tasks, unwinding of cancelled
  tasks, detached tasks at the end of `main`, the 256 KiB task stack) are in `docs/handoff/concurrency.md` on PR #157.

## Decisions (owner, 2026-11-07)

- **R1** Yes. `core` stays a small prelude; OS access lives in `std` (`std.io`, `std.fs`, `std.env`, `std.time`,
  `std.random`, `std.os`). The `std` package now exists on main (`compiler/std/`, #170 to #172); the OS modules join it.
- **R2** Yes. No foreign-library FFI in version 1. The calling convention stays documented for later.
- **R3** Yes. Windows randomness may import `RtlGenRandom` from `advapi32`.
- **R4** Yes. Stop messages carry a source location, in release builds too, encoded compactly (a file id and a line,
  with the names in a side table that is only read when a stop happens) so the binary stays small.

Work starts with package 2 and 3 of section 8 on Linux x86-64, using the thread-side `os_*` names above.
