# OS memory intrinsics (M7)

Status: part 1 (section 1) and part 2 (section 2) and part 4 (section 4, the intrinsic) and the first half of section 3 (per-callee write sets for module functions) built; the runtime routines' own sets are next. The four questions at the end were answered by the coordinator on the owner's delegation (2026-11-07): take the recommendations. Drafted for the owner. Asked for by the heap thread (H6); built on the machine IR (`mir.md`), which is now the default generator.

## What the heap needs

A heap written in Pelago needs four things the compiler does not give it today:

1. Raw pages from the system, with errors the program can test, not a stop.
2. Functions with no prologue, so an allocator fast path costs no more than the inline code it replaces.
3. A fixed rule for which registers a call to an allocator function destroys.
4. Words of static data it can read and write (the heap's own state).

## 1. The intrinsics

**Built.** The names are `rt_os_reserve`, `rt_os_commit`, `rt_os_release`, `rt_os_remap`, `rt_os_advise_nohuge` (every compiler intrinsic in `core` starts with `rt_`) plus `rt_int_addr(n) -> ptr` to turn the returned address into a pointer. They take and give `int` (an address is an `i64` in the IR: `Op::Os(kind, a, b, c)`); a `ptr` argument is converted by the lowering. Addresses and sizes must be whole 4096-byte pages, else `-2`. A system error is `-1` (`ENOMEM`), `-2` (`EINVAL`) or `-3`. The `PELAGO_OS_FAIL` test hook was dropped: the error paths are testable with a size of `1 << 62` (`-1`) and with unaligned arguments (`-2`), see `compiler/core/tests/os_memory.pel`. The interpreter implements the calls on its own heap (a reservation is a zeroed, page-aligned block). `Remap` is `mremap` on Linux and reserve, copy, release elsewhere. x86-64 and AArch64 (Linux, Windows, macOS) routines are in `rt.rs` and `rt_a64.rs`; the Windows and macOS ones are tested by CI only.

All take and give `int` (addresses are `ptr` where the library already uses it) and return a negative `int` on failure, never stop the program. Success is a non-negative address or `0`.

| Intrinsic | Linux | Windows | macOS |
|---|---|---|---|
| `os_reserve(size) -> ptr or err` | `mmap(PROT_NONE, MAP_PRIVATE\|MAP_ANON\|MAP_NORESERVE)` (flags `0x4022`) | `VirtualAlloc(MEM_RESERVE, PAGE_NOACCESS)` | `mmap` (same, `MAP_NORESERVE` is ignored) |
| `os_commit(p, size) -> err` | `mprotect(RW)` | `VirtualAlloc(MEM_COMMIT, PAGE_READWRITE)` | `mprotect(RW)` |
| `os_release(p, size) -> err` | `munmap` | `VirtualFree(MEM_RELEASE)` (size must be 0 there: the intrinsic passes it) | `munmap` |
| `os_remap(p, old, new) -> ptr or err` | `mremap(MAYMOVE)` | no kernel call: reserve, commit, copy, release | same fallback |
| `os_advise_nohuge(p, size) -> err` | `madvise(MADV_NOHUGEPAGE)` | no-op, returns 0 | no-op, returns 0 |

Error values: `-1` for out of memory, `-2` for a bad argument (unaligned or zero size, detected before the call), `-3` when the system says no for another reason. A test mode (`PELAGO_OS_FAIL=n`, the n-th call fails with `-1`) lets the library test its error paths without exhausting memory.

How: one new IR op `Os(kind, args)` that the MIR carries as a call to a runtime routine per kind (the routines live in `rt.rs` and `rt_a64.rs`, next to `rt_alloc`). No new MIR opcode is needed: it is a call with a documented clobber set (section 3). The interpreter implements the kinds with the host's `mmap`.

## 2. Inline-always and zero-prologue

**Built.** The language already has the `@name` attribute (`@checkfree`), so the markers are attributes: `@inline_always` makes the IR inliner copy the function into every call whatever its size (the word `inline` stays banned as a keyword). A recursive function is never copied, and the build fails if a call to an `@inline_always` function is left after optimising. `@leaf` is a checked promise: after optimising, the function makes no call and keeps no stack slot (`opt::is_leaf`), else the build fails. The MIR emitter already omits the frame when nothing is spilled. Both are checked in `main.rs` next to `@checkfree`; they are meant for `core` and the runtime, but nothing stops other code from using them. The answer to question 2 asked to wait for the attribute decision; since `@checkfree` exists, the attribute form costs nothing and the names stay `inline_always` and `leaf`.

## 3. Clobber rule

**Built for module functions** (`native/clobber.rs`, used by `gen.rs` and `gen_a64.rs`). Functions are selected and allocated callee first (postorder of the call graph; a call that closes a cycle is skipped). After a function is allocated, its *write set* is every register an instruction defines or clobbers, minus the callee-saved registers (every function saves and restores those), plus the registers the emitter writes without saying so (`r10`, `r11` for the page probe on x86-64). A direct call in a caller allocated later is narrowed to that set before allocation, so a value that lives across it may stay in any allocatable register outside the set. Calls through pointers, calls that close a cycle and every call of the runtime (`CALLRT`) keep the whole caller-saved set, so a function that prints, allocates or stops has a full set itself. The code is still laid out in module order. Answer 3 of the questions asked to start with `core` and the runtime; the order costs nothing measurable (a postorder walk), so it applies to every function.

**Measured** on the 40 benchmark programs (x86-64): only `levenshtein` (about 9 percent faster, 0.757 s to 0.691 s) and `kostya_json` (no change beyond noise) produce different code. Most hot calls are already inlined, recursive (a cycle), or reach a function that calls the runtime, which has the full set. Two things widen it: exact sets for the runtime routines (`rt_alloc`, `rt_free`, `print`, which today stop every caller from keeping anything), and a set for recursive functions computed to a fixed point instead of the full set.

## 4. Data words

**Built (the intrinsic).** `rt_data_word(k) -> ptr` is the address of the k-th 8-byte word of a block of `DATA_WORDS` (64) zeroed words the compiler adds to the module's static data on first use (`Module::word_block`). `k` must be a literal below 64, so a bad one is a compile error, not a run-time stop. All static data already lives in the writable data section (native) and in interpreter memory, so no new section or op was needed: the address is `DataAddr` plus a constant. Test: `compiler/core/tests/data_words.pel`. The `static name: int` form stays for the module work.

## Questions

1. Are the error values (`-1`, `-2`, `-3`) and the `PELAGO_OS_FAIL` test hook right, or should errors be an `option<ptr>` or `result` in the library wrapper only (the intrinsic stays an `int`)? Recommendation: `int` in the intrinsic, `result` in the wrapper.
2. `#inline` and `#leaf` as attributes: does the language have an attribute syntax yet? If not, should these be a `meta` annotation instead? Recommendation: wait for the attribute decision and use the names `inline_always`, `leaf` as `rt_` intrinsic-only markers in `core` until then.
3. Per-callee clobber sets (section 3) change allocation order for the whole program and could cost compile time. Take the bottom-up order only for functions in `core` and the runtime first? Recommendation: yes, measure, then widen.
4. Data words: `static` at module level in `core`, or the `rt_data_word(k)` intrinsic? Recommendation: the intrinsic now (no syntax), `static` later with the module work.
