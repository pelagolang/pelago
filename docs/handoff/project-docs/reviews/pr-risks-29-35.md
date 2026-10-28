# Risks stated in pelagolang/pelago PRs #29 to #35

Read-only review, 2026-10-20 ~13:15 UTC. Sources: PR bodies (no review comments exist on any of them), the CI check runs, and `compiler/src/native/rt.rs` on main (01487d7, which has #33 but not #32, #34, #35).
"Inferred" marks my own reading, not something a PR states.

## Status table

| PR | State | Stated risk | Addressed on main? |
|---|---|---|---|
| #29 accumulator recursion | merged | Sum is regrouped: with mixed signs a checked `+` can stop at a different addition, or not stop where the original did | **No.** Fixed by #32, still open. Main has the unsound version until then |
| #30 CSE, cheap constant division | merged | None for semantics. divmod still 1.02x C -O2 (four overflow checks per iteration) | Semantics fine. Perf gap remains |
| #31 float32 rounding, saturating float to int | merged | None stated | Yes. Nothing outstanding found |
| #32 exact accumulator | open | Windows and Linux CI not yet green when written | Windows job still running at last look; Linux `check` green |
| #33 buffered print | merged | A program killed by a signal loses unflushed output | **Not yet.** Fix is #35 (open). Also inferred: on main `rt_stop` writes the message to stderr before `rt_exit` flushes stdout, so order is inverted until #35 |
| #34 realloc with mremap | open | Windows has no `mremap`: keeps copy path (no speedup). list_sum 1.27x and strbuild 1.41x still above C -O2 | No. CI running |
| #35 stack and bad-address stop | open | Windows handler never run (first run is CI). A frame bigger than the guard zone can step over it | No. CI running |

## Open risks and proposed solutions

### 1. #29 regrouped checked sum (fix: merge #32)
#32 already implements the fix: a guard `(x | (acc + x)) < 0` keeps the loop only while terms are non-negative and cannot overflow the prefix; the first failing term leaves through a cold path that replays the original order. Tests: 216 sign mixes, named i8 cases. Cost measured as nothing (fib38 0.20 s vs 0.21 s).
Action: merge #32 once Windows CI is green. Until then main can stop at the wrong addition. Remaining perf: fib38 still ~1.3-1.7x C/Rust (call overhead). Next lever named in the PRs: inline the recursive call a level or two.

### 2. #34 Windows has no mremap
There is no remap-without-copy on Windows (VirtualAlloc cannot move or extend a region in place). Options, best first:

**A. Reserve ahead, commit as it grows (recommended).** For a large block, `VirtualAlloc(NULL, reserve, MEM_RESERVE, PAGE_READWRITE)` for a generous range (e.g. 16x the requested size, minimum 64 MiB; reserve costs no commit charge), then `VirtualAlloc(base, newlen, MEM_COMMIT, PAGE_READWRITE)` on the same base to grow. Nothing is copied or moved; committed pages arrive zeroed, which matches `rt_realloc`'s "rest is zero". The header keeps both lengths (committed and reserved), so the 16-byte header grows or the reserved length goes at offset 16 of a larger header. Grow = one syscall, and the callers already double, so that is O(log n) calls. `VirtualFree(base, 0, MEM_RELEASE)` frees the whole reservation. When the reservation is exhausted, fall back to alloc + copy + free into a new, larger reservation (rare).
 - Watch: the freed-large-block reuse slot (state offset 120, #24) compares sizes via the header; it must compare committed size. Shrink can be a no-op (or `MEM_DECOMMIT` of whole pages).
 - Cost: address space only (fine on x64) and 64 KiB reservation granularity.

**B. HeapReAlloc / HeapAlloc.** Imports `GetProcessHeap`, `HeapAlloc`, `HeapReAlloc`, `HeapFree`; `HEAP_ZERO_MEMORY` gives zeroed growth. Simple, but large blocks still go through the heap's own VirtualAlloc and are copied when they cannot extend in place, so no guaranteed win, and it replaces our own allocator semantics (leak counts, header layout). Not recommended.

**C. Placeholders (`VirtualAlloc2` + `MEM_REPLACE_PLACEHOLDER`, `MapViewOfFile3`)** can remap pages without copying but need Windows 10 1803+ and a lot of machinery. Not worth it.

Also needed: a Windows benchmark number. The suite compares against gcc on Linux only, so nobody knows today how much the copy path costs there. Run `compare.sh` equivalents on the Windows CI runner before and after A.

### 3. #34/#35 both edit `native/rt.rs`
Inferred: different regions (realloc routine vs start-up and handlers), but the shared state table and import list (`IMPORTS`, `STATE_SIZE`) will conflict. Merge one, then rebase the other onto main (merge, not force-push).

### 4. #35 untested Windows handler
Stated: CI is the first run. Proposed handling: keep the PR open until the Windows job passes; if it fails, fix forward as the PR says. Two things to check in that run: the vectored handler decides stack overflow by fault address within the guard zone; and the fallback path when the 256 MiB `VirtualAlloc` fails (the system stack stays but the guard page is placed relative to the custom region; inferred, worth a test with a forced failure).

### 5. #35 frame larger than the guard zone
Stated limit; the lowering has no such frames today. Proposed hardening: (a) Windows guard is a single page 64 KiB above the base, so any frame over 4 KiB that touches below the guard page first can skip it. Emit a stack probe (touch each 4 KiB page, like `__chkstk`) in the prologue of any function whose frame exceeds 4 KiB, or make the lowering assert a frame limit; (b) Linux's 1 MiB guard is much larger, so a compile-time diagnostic "frame bigger than 1 MiB" is enough there.

### 6. #35 Windows commits the whole 256 MiB stack at start (inferred)
`_start` calls `VirtualAlloc(MEM_COMMIT | MEM_RESERVE, 256 MiB)`. That charges 256 MiB commit for every program, including hello world, and can fail on small-pagefile machines. Proposal: reserve 256 MiB, commit only the top chunk, and let the guard-page handler commit the next chunk and move the guard down; report stack overflow only when the reserve is used up. Same mechanism the OS uses for its own stack.

### 7. #33 buffered print side effects (inferred)
Output only appears at 64 KiB, exit or stop, so a program that loops forever or is killed by a timeout shows nothing, and stdout/stderr ordering differs from the interpreter. #35 handles faults and ordering for `stop`. Still open after #35: hard kills (SIGKILL, timeouts) and any future stdin reading (flush before blocking read). Proposal: flush before a blocking input call when input exists, and consider line-flush when stdout is a terminal (`isatty` ioctl on Linux, `GetConsoleMode` on Windows).

### 8. Performance gaps still stated against the "at or below C -O2 and Rust -O" goal
- fib38: 0.20 s vs C 0.15 s / Rust 0.14 s (#32)
- tak: ~1.25x C -O2 (#29)
- divmod: 1.02x C -O2 (#30)
- list_sum 1.27x, strbuild 1.41x vs C -O2 (#34): the push loop keeps `len` in memory because the slow call may change the header. Proposal: keep `len`/`cap` in registers across the push loop and reload only after `rt_grow_slow`.
- Geometric mean vs C -O2: 1.45x after #33.

## Done
#29 (apart from the semantics hole), #30, #31, #33 are merged with their tests and Windows CI green per their bodies. #31 and #30 state no open risk and I found none.

## Update 2026-10-20 16:40 UTC: Windows benchmark landed (#36)

The `bench-windows` job (GitHub `windows-latest`, MinGW gcc, rustc stable, best of 5) is on main. First baseline, before any VirtualAlloc growth change:

| program | pelago (s) | C -O2 (s) | Rust -O (s) | vs -O2 | vs Rust |
|---|---|---|---|---|---|
| collatz | 0.0596 | 0.0557 | 0.0478 | 1.07x | 1.25x |
| divmod | 0.0315 | 0.0407 | 0.0396 | 0.77x | 0.80x |
| fib38 | 0.1067 | 0.0764 | 0.1249 | 1.40x | 0.85x |
| floatloop | 0.1222 | 0.1213 | 0.1181 | 1.01x | 1.04x |
| list_sum | 0.0234 | 0.0327 | 0.0343 | 0.71x | 0.68x |
| loop_sum | 0.3246 | 0.3260 | 0.3283 | 1.00x | 0.99x |
| mapwork | 0.0161 | 0.0136 | 0.0231 | 1.18x | 0.70x |
| sieve | 0.0586 | 0.0497 | 0.0477 | 1.18x | 1.23x |
| strbuild | 0.0098 | 0.0108 | 0.0150 | 0.91x | 0.65x |
| structloop | 0.0170 | 0.0171 | 0.0103 | 0.99x | 1.65x |
| tak | 0.0931 | 0.1688 | 0.1806 | 0.55x | 0.52x |

Geometric mean 0.95x vs C -O2 and 0.89x vs Rust; at or below C -O2 on 6 of 11. Shared runner, so noise is a few percent.
Note: list_sum and strbuild are already below C on Windows without any remap (the Windows allocator path differs from Linux's), so the VirtualAlloc reserve and commit change should be judged on these two rows plus a larger push test.

Lessons recorded: the C twins used `long` (32 bits on Windows) and hung collatz; they now use `long long`. C's text-mode stdout writes CRLF, so outputs are compared with CRLF read as LF.
