# Replacing the heap allocator (draft 1)

Question from the owner (2026-10-27): programs still claim a lot of memory (a fixed 2 MiB floor, C needs about 0.8 MiB), so is there a
smarter way to reserve and use memory, and should Pelago drop the heap allocator and the heap altogether? This page answers it, reads the
runtime as it is, surveys the options, and recommends one design with stages. Nothing here is measured by a new run; where a number is
an estimate it says so. Related: [arena.md](arena.md) (A-13), `docs/plans/cards/` (narrow links, PR #139), [concurrency-runtime.md](concurrency-runtime.md).

## 1. Where the memory goes today

Read from `compiler/src/native/rt.rs` (the AArch64 runtime `rt_a64.rs` is a second copy of the same design).

| Thing | What the code does | Effect |
|---|---|---|
| Start-up stack | 1 GiB lazy `mmap`, then `madvise(MADV_HUGEPAGE)` | **The 2 MiB floor.** Touching the top of the stack faulted in a whole 2 MiB page. The narrow-links thread found this (`hello` 2072 KiB) and fixed it in PR #138 (1116 KiB, C 1300 KiB). I did not re-measure |
| Print buffer, signal stack | 64 KiB and 32 KiB mappings | Small; the print buffer is only mapped when the program prints |
| Small blocks (up to 64 KiB) | Size classes that **double** (32, 64, 128, ...), a free list per class, a 16-byte header per block, carved from 1 MiB chunks | Internal waste of 25% to 50% on average plus 16 bytes per block. A `box<tree>` node needs 16 + 25 bytes and gets a **64-byte** block (C: 24 to 32) |
| Reuse | A reused block is cleared with `rep stosb` | Startup cost of `rep stosb` on tiny blocks is a large part of an alloc. It exists only because the interpreter hands out zeroed memory (A-11 S5 already lists dropping it) |
| Large blocks | Own mapping, rounded to 4 KiB; from 2 MiB up rounded to 2 MiB and advised huge; one freed block is cached | Up to 2 MiB of extra resident memory per large block, and a syscall per growth step on the miss path |
| Allocation count | `lists`, `maps`, `text`, `objects`, `boxes`, `tasks` call `rt_alloc` or `rt_realloc` in 45 places | Every growable buffer, every `box`, every int-to-text temporary goes through the same general path |

So "ditch the heap" is aimed at two real costs: the **general-purpose path** (class search, headers, zeroing, a call) and the **resident
floor** (huge pages, rounding). The first stage of the answer is small and already in flight.

## 2. What "no heap" can and cannot mean

Pelago has `list`, `string`, `map`, `box`, `sink` moves and `pop`/`remove`/`insert!` on containers. Data of unknown size that dies in any
order needs a place to live and a way to give it back, so some allocator must exist. What can go:

- the **libc-style** allocator (headers, searched classes, zeroing, one path for everything);
- the **assumption of one heap**: lifetimes that are provably last-in-first-out do not need free lists at all.

Pure scope stacks (every allocation is released when its scope ends) do not fit, for a reason that is specific to value semantics:
`fn make(d) -> tree` allocates boxes in the callee and **returns** them. On a single bump stack the result is buried under the callee's
temporaries, so it must be copied down on return (a deep copy per level: `bintrees` would copy each tree 16 times) or the callee must write
into a region chosen by the caller (destination passing, which only works when the size is known). A container that does `remove`
in the middle frees out of order. So a stack-only design needs either hard limits (no recursion deeper than N, no removal) or a fallback
allocator. We keep Pelago's rules (exact overflow traps, no hidden limits), so the answer is a **hybrid with no libc-style heap**.

## 3. Options

| Option | How it works | Fits Pelago? | Verdict |
|---|---|---|---|
| A. Goose-style bump stacks only | One data stack per task; a scope end resets the top | Breaks on returned values and out-of-order frees (section 2); needs limits | Not as the only mechanism. Take the idea for provable temporaries (E) |
| B. Region or arena (explicit) | A block you bump into and reset | Yes, as a library type; no lifetimes needed with epoch handles | Already designed: [arena.md](arena.md). Keep, build on C |
| C. **Reserve-and-commit address space, spans, finer classes** | Reserve one big virtual range at start (free of charge, nothing resident), commit in steps as the top moves, carve fixed-size spans per class | Yes. Same `Alloc`/`Free`/`Realloc` IR, no language change, no checker rule | **Core of the recommendation** |
| D. Size-class free lists as today | | Works, but see the table in section 1 | Keep the free-list idea, drop the header, the doubling classes and the zeroing |
| E. Escape analysis to the stack | An allocation whose pointer never leaves the function and is freed on every path becomes a stack slot or scratch-stack block | Easy here: ownership already says who frees. A value dropped at its own scope end and never `sink`ed out is LIFO by construction | Yes, for short-lived temporaries (interpolation, int-to-text, sort scratch, small `box`es kept local). Does **not** help `bintrees` (nodes escape) |
| F. Tracing or reference counting | | Contradicts "no GC, no runtime" | No |
| G. Per-worker heaps with copy-on-send channels | Each worker owns a sub-range; values never cross | Yes, and it removes the usual cost of a shared heap: a value is never freed by another thread, so no atomics and no remote-free queue | Yes, at concurrency stage C6 |

## 4. Recommendation: one reserved range, spans, no header

**Reserve.** At the first allocation the runtime reserves one large range with no access (Linux: `mmap` with `PROT_NONE`; Windows:
`VirtualAlloc(MEM_RESERVE)`). It costs no memory. A bump pointer `top` and a `committed` mark live in the runtime state. When `top`
crosses `committed`, the slow path commits the next 1 to 4 MiB (`mprotect`, `VirtualAlloc(MEM_COMMIT)`). Nothing is advised huge by
default.

**Spans, not headers.** The range is cut into 64 KiB spans. A span belongs to one size class; a one-byte table indexed by
`(addr - base) >> 16` names the class. `free(p)` is: subtract, shift, load the byte, push on that class's free list. That removes the
16-byte header (`box<tree>` 64 B -> 32 B; 24 B if the `+ 1` in `rt_box_new` is dropped for sized types), and the table lives in the
same reservation. Classes step by 16 B up to 256 B, then 4 per doubling to 64 KiB (at most 12.5% waste). Larger blocks get whole spans,
**grown in place** by committing more of the reserved neighbour (the Windows path already does this with an 8x reservation; Linux
`mremap` becomes a fallback).

**Fast path.** A class free-list pop, else a `top` bump with a bound compare. No zeroing: fresh pages are zero by the kernel, and the
lowering writes before it reads (the A-11 S5 audit decides the exceptions). It is small enough to emit **inline** by the backend (a
sequence in the machine-level IR of the W4 thread) instead of a call.

**Whole-program specialisation (binary size).** The runtime already skips `rt_free` when no program path frees (`free_used`). Extend it:
no free anywhere means the allocator is `top += n` (about 30 bytes); one size class means one free list; no growth means no
`rt_realloc`. A program with no allocation at all gets no reservation and no allocator, which is the literal "no heap" case and the
`hello` case. This is where tcc-small comes from; the general path only costs what the program uses.

**Arenas on top.** An arena (A-13) reserves its own range, say 1 GiB virtual, and commits as it grows, so `new` never moves anything
and `reset!` is `top = 0` (plus an optional decommit). The 32+32-bit handle in arena.md is unchanged.

**Scratch for temporaries.** The lowering turns a provably local allocation (pointer used only inside the function, freed on all paths,
at most a few KiB) into a frame or scratch-stack slot. First targets: the 24-byte digit buffer in `text.pel`, interpolation temporaries,
`rt_swap_bytes`, `rt_sort` scratch below a size limit.

**Concurrency.** Each worker gets its own sub-range (`addr >> 34` names the owner); free lists are per worker, no locks. A task stack
is a reserved block taken from a stack pool in the range (256 KiB reserve, lazily committed, guard page), which is the `os_stack_alloc`
of the concurrency plan. Copy-on-send channels mean no value is freed by a different worker than the one that allocated it, except a
`spawn` argument moved in; copy that or hand it back through a one-slot return queue.

**No-libc boundary.** The whole allocator needs three OS services: `os_reserve(size) -> ptr`, `os_commit(ptr, size) -> bool`,
`os_release(ptr, size)`. Linux is `mmap`, `mprotect`, `munmap`; Windows is `VirtualAlloc`, `VirtualAlloc`, `VirtualFree`. This is the
smallest boundary the runtime period needs. Once W4 settles, the allocator can be written **in Pelago** (`core/rt.pel`) on top of
these, which replaces the two assembly copies (x86-64 and AArch64) with one source and lets the optimiser inline it.

**Risks.**
- `RLIMIT_AS` (`ulimit -v`) counts reservations, and strict overcommit refuses some mappings: reserve in a loop that halves the size until it succeeds, and stop with `out of memory` only when the first commit fails.
- Address space is large but not infinite (39-bit on some ARM systems): size the reservation from what the OS grants, never assume 1 TiB.
- Commit steps are syscalls: 1 to 4 MiB steps keep them rare; a program that grows to 1 GiB makes a few hundred.
- Span tables add a lookup on free; benchmark `bintrees` before and after, with the one-row rule below.

## 5. Stages

Each stage is its own PR on its own branch and names the one row it moves; no full benchmark suite runs.

| Stage | What | Moves |
|---|---|---|
| H0 | Stack floor fix (PR #138), record the start-up baseline | `hello` memory |
| H1 | Region heap with the same ABI: reserve and commit, spans, finer classes, no header, no zeroing on reuse, no default huge pages (x86-64 first, AArch64 after) | `bintrees` time and memory, every row's memory |
| H2 | Whole-program specialisation of the allocator (no free, one class, no growth, no allocation) | binary size, `hello` |
| H3 | Scratch allocation for provably local temporaries | `strsplit` |
| H4 | Arenas on the reserved ranges (A-13 AR2 to AR4) | `bintrees` arena twin |
| H5 | Per-worker sub-ranges and the task stack pool (concurrency C1, C6) | concurrency rows, none today |
| H6 | The allocator written in Pelago, inlined by the optimiser | all alloc-heavy rows, one source for both targets |

H1 and H2 are independent of the arena and narrow-links work. Narrow links (PR #139) shrink the data the allocator hands out
(`bintrees` nodes about 3x) and compose with H1: smaller blocks, no header. The arena thread's AR5 (lazy `str.chars()`) is untouched.

## 6. Expected effects (estimates, not measurements)

| Row | Today | With H1 to H3 | Why |
|---|---|---|---|
| `bintrees` | 4.7x behind the best C row | roughly 2x behind; about level with an arena twin (H4) | Node block 64 B -> 32 B (cache and memory half), alloc is an inline pop or bump, no `rep stosb`, no call. C's malloc still pays its own cost |
| `mapwork` | 8x behind | 10 to 20% better, no more | Cost is the map design (S-01), not the allocator. Rehash tables are large blocks: fewer page faults and no huge-page rounding help a little |
| `strsplit` | 4x behind | maybe 1.3 to 2x better | A temporary string per `pushStr!("#i ")` (300000 alloc/free pairs) goes to scratch (H3); the one growing buffer already grows in place |
| `lexer` | 5x behind | **no change** | The cost is `str.chars()` building a full list (60000 lines of about 40 chars, so millions of entries). The fix is the lazy iterator in A-13 AR5, not the allocator. Memory use falls from tens of MiB to nothing then |
| `hello` memory | 2072 KiB -> 1116 KiB (H0) | no further change expected | The remaining floor is the process itself |
| Binary size | 3.5 to 20 KB | `hello` 20 to 30% smaller if the allocator is dropped; programs that never free save the free-list code | Estimate from the size of the allocator routines; H2 measures it |

## 7. Questions for the owner

1. **Literal "no heap", or "no libc-style heap"?** Recommend the second: keep a tiny region allocator for the data that must outlive its scope, and make it vanish in programs that do not allocate (section 2 says why a pure stack design needs limits).
2. **Reservation size.** Recommend asking the OS for as much as it grants up to 256 GiB for the program and 1 GiB per arena, halving on refusal. Alternative: a fixed 64 GiB.
3. **Zeroing.** Recommend dropping "an allocation is zeroed" as a language rule; the lowering must write before it reads. The interpreter keeps zeroing, so it stays the reference.
4. **Where the allocator is written.** Recommend assembly now (H1) and Pelago later (H6), after the machine-level IR lands.
5. **Huge pages.** Recommend off by default, opt-in per large block with a build flag.
