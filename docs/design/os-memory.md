# OS memory intrinsics (M7)

Status: draft for the owner. Asked for by the heap thread (H6); built on the machine IR (`mir.md`), which is now the default generator.

## What the heap needs

A heap written in Pelago needs four things the compiler does not give it today:

1. Raw pages from the system, with errors the program can test, not a stop.
2. Functions with no prologue, so an allocator fast path costs no more than the inline code it replaces.
3. A fixed rule for which registers a call to an allocator function destroys.
4. Words of static data it can read and write (the heap's own state).

## 1. The intrinsics

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

An attribute `#inline` on a function (the word `inline` stays banned as a keyword; this is an attribute, not syntax) makes the IR inliner always take it, whatever its size, and refuse recursion. A function marked `#leaf` promises no calls and no stack frame beyond what the register allocator can keep in registers; the MIR emitter then skips the frame setup when the allocator spilled nothing (it already does this for frameless leaf functions; the attribute turns a silent optimisation into a checked promise: the build fails if the promise cannot be kept).

## 3. Clobber rule

Today a call destroys every caller-saved register. The rule becomes per callee: the MIR keeps a set for each function, computed bottom-up after allocation (the registers the function writes, plus those of its callees; an unknown callee, a call through a pointer or a recursive cycle destroys the whole caller-saved set). A caller then keeps values live across calls in registers the callee does not touch. The runtime routines get fixed documented sets (`rt_alloc`: `rax`, `rcx`, `rdx`, `rsi`, `rdi`, `r8`-`r11` on x86-64 until it is rewritten in Pelago, then whatever the allocator says). This is the part that needs the most care: the set must be known before the caller is allocated, so functions are allocated callee-first (the call graph's order, cycles broken at their entry).

## 4. Data words

`DataAddr` already gives the address of static data. What is missing is a writable place: today data lives in the read-only section for strings and tables. The proposal: `static name: int = 0` at module level in `core` only (a language feature the library uses, not users), placed in a read-write data section; `name` reads and `name = v` writes. Alternative without syntax: an intrinsic `rt_data_word(k) -> ptr`, the address of the k-th word of a block the compiler reserves (size set by a constant). The intrinsic is less pretty and needs no parser change.

## Questions

1. Are the error values (`-1`, `-2`, `-3`) and the `PELAGO_OS_FAIL` test hook right, or should errors be an `option<ptr>` or `result` in the library wrapper only (the intrinsic stays an `int`)? Recommendation: `int` in the intrinsic, `result` in the wrapper.
2. `#inline` and `#leaf` as attributes: does the language have an attribute syntax yet? If not, should these be a `meta` annotation instead? Recommendation: wait for the attribute decision and use the names `inline_always`, `leaf` as `rt_` intrinsic-only markers in `core` until then.
3. Per-callee clobber sets (section 3) change allocation order for the whole program and could cost compile time. Take the bottom-up order only for functions in `core` and the runtime first? Recommendation: yes, measure, then widen.
4. Data words: `static` at module level in `core`, or the `rt_data_word(k)` intrinsic? Recommendation: the intrinsic now (no syntax), `static` later with the module work.
