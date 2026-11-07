# Why Pelago binaries are bigger than assembly or tcc, and the plan to fix it

Question from the owner (2026-11-07): native binaries are far bigger than hand-written assembly or `tcc` output. Why, and how do we get to
tcc-small, with IoT and microcontroller targets in mind (few registers, small flash and RAM)? This page is a measurement (x86-64 Linux, `pelago
build`, default flags, 2026-11-07, commit 532dc48) and a ranked plan. Nothing here changes the compiler yet. The numbers marked *est.* are
estimates read from the disassembly, not measured after a change.

## Short answer

Pelago's programs are small already when judged by what `gcc` needs for a hello world (`gcc -Os` dynamic: 14.5 KB, static: 706 KB), but they
are big against what the program itself needs, because **a fixed 1 KB runtime, 0.6 to 0.8 KB of debug tables, 0.3 KB of zeros stored as
data, 0.1 to 0.5 KB of alignment padding and 6-byte branches are in every binary whatever the program does.** For `hello` that is 2608
bytes of which about 60 are the program. About 60 percent of every small binary is removable without touching the generated code of user
functions, and most of it is cheap.

## Measured breakdown

Bytes in the file. `trailer` = section headers, symbol table and string tables after `.data` (tools only; the loader ignores them).
`zeros` = zero bytes stored inside `.data` (the runtime state and heap tables; they belong in `.bss`, which costs no file bytes).
`user` = sum of the program's own function symbols (`nm -S`). `nop pad` = alignment `nop`s in `.text`.

| program | file | header gap | .text | .rodata | .data | zeros in .data | trailer | user fns | nop pad |
|---|---|---|---|---|---|---|---|---|---|
| hello | 2608 | 256 | 1179 | 132 | 393 | 327 | 631 | 43 | 112 |
| sieve | 2800 | 256 | 1381 | 132 | 377 | 324 | 631 | 437 | |
| fib38 | 3112 | 256 | 1671 | 132 | 377 | 324 | 655 | 635 | |
| bintrees | 5936 | 256 | 4358 | 175 | 377 | 324 | 759 | 2128 | 459 |
| lexer | 6528 | 256 | 4919 | 145 | 441 | 365 | 743 | 2463 | 486 |
| strsplit | 8016 | 256 | 6391 | 160 | 503 | 369 | 697 | 3964 | 555 |

Other flags, same programs: `--no-opt` gives 10168 (hello), 12208 (bintrees), 12976 (lexer): the optimiser's pruning of unused library
code is already the biggest single lever and works. `--heap=pelago` gives 3120, 8408, 9672: the language-written heap costs 0.5 to 3.1 KB
more than the assembly allocator (docs/design/heap.md, H6c).

### What is inside `hello` (1179 bytes of code, 43 of them `main`)

Read from `objdump -d`; the runtime is emitted by `native/rt.rs` whatever the program uses.

| part | bytes | needed by hello? |
|---|---|---|
| `_start`: 1 GiB stack `mmap` plus `madvise(HUGEPAGE)` | 88 | no (no recursion) |
| `_start`: 64 KiB print buffer `mmap` plus tty probe `ioctl` | 114 | the buffer could be `.bss`; the probe is optional |
| `_start`: guard-region unmap, `sigaltstack`, two `sigaction`s (stack overflow and bad access) | 207 | no |
| fault handler (prints "stack overflow" or "invalid memory access") | 80 | no |
| `print` 125, `flush` 51, `write_fd` 47 | 223 | yes (about 120 of it) |
| `stop` (prints "stopped: ...", exits) | 81 | no |
| `free` and its large-block path | 137 | no (hello never allocates) |
| four stubs that print the task and deadlock messages | 68 | no |
| alignment `nop`s before `main` (functions start 48 bytes into a cache line) | 112 | no |
| exit sequence and `main` call | 28 | yes |
| `main` | 43 | yes |

Outside the code: 132 bytes of `.rodata` (the stop, trap and deadlock messages, most unused), 393 bytes of `.data` (327 of them zero), 24 bytes
between the program headers and `.text`, 12 between `.rodata` and `.data`, and the 631-byte trailer, of which 512 bytes are eight
section headers.

### Where the user code itself is fat (bintrees, lexer, strsplit)

- **Every branch is long.** All 64 conditional jumps in `bintrees` (97 in `lexer`, 104 in `strsplit`) are the 6-byte `jcc rel32`, and
  every `jmp` is 5 bytes: the assembler never relaxes to the 2-byte forms. About 380 to 620 bytes of conditional jumps plus 130 to 265 of
  `jmp`s per program; most of them reach targets within 127 bytes.
- **Moves are 35 to 45 percent of all code bytes.** In `lexer`, 120 of 394 `mov`s are register to register (call-convention shuffles and
  copies the allocator left) and 90 are stack spills; in `strsplit`, 247 of 517 are stack moves. Spills are size *and* speed, and they
  get worse on a register-poor chip, so cutting them is the same work as the "speed without more registers" goal.
- **Overflow checks** are one `jo rel32` (6 bytes) to one of four shared stubs, so they are already cheap in bytes (20 to 23 of them per
  program, about 130 bytes). The trap stubs are shared and not a problem; the 6-byte jump form is.
- **About half of the text is not user code.** `bintrees`: 4358 bytes of text, 2128 in user functions, the rest runtime, allocator and padding;
  `lexer`: 4919 and 2463. The allocator is inlined into the callers (`make` is 1252 bytes in `bintrees`; `gcc -Os` needs 428 for the whole
  translation unit but gets `malloc` from libc, so that is not a like-for-like row).

### Comparison

| | hello | note |
|---|---|---|
| Pelago (this commit) | 2608 | static, no libc, no relocations |
| hand assembly (`write` + `exit`, linked with `-nostdlib`) | 119 bytes of code (856 file with the default linker alignment; a hand-made ELF is about 200) | measured here |
| `gcc -Os`, dynamic, stripped | 14472 (text 1370) | needs libc.so at run time |
| `gcc -O2 -static`, stripped | 706576 | measured here |
| `tcc` | not measured | building tcc from a downloaded tarball was not allowed in this session; run `bench/harness` (it has a `tcc` row) or the Docker toolchain |

Program code only (`gcc -c`, `size` text): sieve 206 (`-Os`) / 276 (`-O2`), fib38 211 / 1465, lexer 440 / 565, strsplit 323 / 389. Pelago's
user functions are 437, 635, 2463 and 3964 bytes. Those C numbers leave out libc (`printf`, `malloc`), so they overstate the gap;
the point is the order: at `-Os` C is about half of Pelago's code, at `-O2` they are close.

## Ranked plan

Savings are per binary, from the table above. "Own idea" marks what uses Pelago's own guarantees instead of copying C, C++ or Rust; the rest
(branch relaxation, `.bss`, dropping debug tables, dead-code removal) is conventional and says so.

| # | item | saves | effort | kind |
|---|---|---|---|---|
| 1 | **No section headers or symbols by default** (`e_shnum` 0); keep them behind `--symbols`/`-g`, and keep the symbol list in a side file for the profiler | 631 to 759 (10 to 24 %) | very small (`elf.rs`) | conventional |
| 2 | **Zeros to `.bss`**: put the runtime state last in `.data`, or in a second `bss` blob, and pass the `bss` argument that `elf::write` already has | about 325 to 370 | small | conventional |
| 3 | **Prune the runtime per program**, not per feature set: drop `free`, the deadlock/task stubs and their strings when no `Free` or task exists; make the stop messages table-driven so unused ones vanish | about 300 for `hello`; less for larger programs | small to medium (`Uses` in `rt.rs` already does this for six flags) | conventional |
| 4 | **Static stack bound instead of a 1 GiB stack and a signal handler.** When the call graph has no cycle and no indirect call, the deepest stack is known at compile time: check it against the stack size once, and drop the stack `mmap`, the guard, `sigaltstack`, both `sigaction`s and the fault handler. For recursive functions, a prologue `cmp rsp, [limit]` / `jb stack_overflow` (no register needed, about 8 bytes per recursive function) replaces the guard page and also works on chips with no MMU and no signals | about 375 for non-recursive programs | medium (call graph in `lower`/`mir`) | **own idea** (exact, language-level "stopped: stack overflow", not a SIGSEGV) |
| 5 | **Output buffer in `.bss`, sized per program** (default 4 KiB on small targets, a build setting) and no tty probe unless asked (`--line-buffered`); a program that never reads a terminal state does not need the `ioctl` | about 100 | small | conventional (the sizing is ours) |
| 6 | **Function alignment**: the 48-byte nop pad per function (`gen.rs`, justified by `fib` and `tak`) costs 112 to 555 bytes. Keep it only for functions the optimiser marks hot (loops, `@inline_always` callees, recursion), pack the rest at 1-byte alignment; `--opt-size` turns it off everywhere | 110 to 550 (8 to 10 % of text) | small | conventional |
| 7 | **Branch relaxation** to `rel8` for jumps and jumps to the stubs; place the four trap stubs in an island per function (or per 120 bytes of code) so `jo` reaches them in two bytes | est. 250 to 500 per program (about 7 percent of text) | medium (`x86_emit.rs`, needs a fixed point because sizes change) | conventional |
| 8 | **Tighten the header**: overlap the program headers and the start of `.text`, drop the padding between segments, and use one `LOAD` segment when there is no writable data beyond `.bss` | about 60 to 100 (hand ELFs are about 120 bytes of header) | small | conventional |
| 9 | **Compact stop messages and source locations** (already decided, 2026-11-07): one byte of reason code per stub, the text printed from one table, locations as 2-byte file/line ids | the 132 bytes of `.rodata` become about 40 plus the table the program really uses | medium | **own idea** (the runtime knows the reasons: overflow, division, shift, bounds, unreachable, stack, out of memory) |
| 10 | **Smaller code from the allocator**: fewer reg-reg moves around calls (callee-write sets are done, #197 and #199), 32-bit operand forms where a value is proven to fit (`byte`, `bool`, range-limited loop counters drop the REX prefix and the 8-byte immediates), spill slots shared between disjoint live ranges, and rematerialise constants instead of spilling them | est. 5 to 10 % of user code (476 to 731 REX-prefixed instructions per program) | large (MIR) | partly **own** (ranges come from the type system and `checkfree`) |
| 11 | **Heap to assembly size**: see below | 0 for `hello`; 2.4 to 3.1 KB less than `--heap=pelago` for programs that allocate | large | **own** (regions, sized free) |
| 12 | **`--opt-size`**: a build mode that disables inlining of calls above a threshold, unrolling, vectorising and the bounds-check duplication, and picks the shortest encodings; the default stays fast | est. 15 to 30 % of user code | medium | conventional |

### Projected sizes (est.)

| program | now | after 1 to 3 | after 1 to 6 | after 1 to 8 |
|---|---|---|---|---|
| hello | 2608 | about 1300 | about 600 | about 450 |
| bintrees | 5936 | about 4600 | about 3700 | about 3300 |
| lexer | 6528 | about 5100 | about 4000 | about 3500 |

Items 1 to 3 are the cheap, safe ones: they do not change a single instruction of user code. Item 4 plus 5 plus 6 are what make `hello` tiny.
A hand-made hello is about 200 bytes with a header, so "tcc-sized" (tcc programs are 2 to 7 KB before libc) is reachable with items 1 to 6;
"assembly-sized" for hello is reachable only by also dropping the buffered `print` for small targets.

## The heap (asked in the same message)

The language heap (`--heap=pelago`) is about 460 IR operations (about 2.7 KB): `heap_alloc` 170, refill 85, free 64, init 62, commit 44, large 37.
The assembly allocator is about 700 bytes including `realloc`. Speed is equal, so size is the only reason it is not the default
(decision 2026-11-07: no platform split, shrink it, then make it the default everywhere). Plan, in order:

1. **One size-class lookup, no span table.** The 2.7 KB comes mostly from handling small, medium and large blocks as three designs. Make
   it a bump region plus one free list per class, with the class a constant at every `Alloc` whose size is constant (H6c a), so the fast path is
   a pop or a bump and the slow path is one routine. Own idea: a `drop` knows the static type, so `free` takes the class (sized free), and the span table
   and its lookup (`movzbq 0x800(...)`, about 100 bytes of the runtime) disappear.
2. **One OS call routine.** `init`, `commit`, `large` and `refill` each do their own `mmap` setup. Share one `heap_map(len)` and let
   `large` be "a class of one block" in the same free-list code (about 150 to 250 IR operations less).
3. **Check whether `realloc` earns its place in the heap**: if growth (`list`, `string`) is its only user, make it `alloc` + copy + `free` in `core`
   so the heap has two entry points (to verify before doing it).
4. **Drop it entirely where it is not needed** (arena or region programs that never free): no `free` routine is linked (the pruning in item 3).
5. Measure `bintrees`, `strsplit`, `mapwork` and `lexer` with `pelago build` sizes (not the harness) after each step. Target: within 100 bytes of
   the assembly allocator on every row, then H6d.

## Speed without more registers (asked in the same message)

Size and speed meet here: a spill is code bytes and a memory access. What helps on x86-64 with 16 registers helps more with the 6 to 8 usable on a
small core, and none of it needs a register:

- **Do not inline when it adds register pressure.** The inliner and the allocator should count the live values at the call site; when the
  function would not fit, keep the call (`--opt-size` and the register-poor target both use that rule).
- **Fewer live values by construction** (own idea): `inout` and `sink` give exclusive access, so a loop that updates a field of an `inout`
  value can keep it in a register for the whole loop and write it back once, without alias checks (the current push-loop guards prove this
  at run time). Bounds-check elimination and `checkfree` loops (Goose comparison, adopted) remove the index plus length pair from the live set.
- **Narrow values** (byte types, `wrapToByte`): 32-bit and 8-bit forms are shorter and let two values share a register.
- **Constants as immediates, not registers**: the division by constant (multiply) and compare-with-immediate paths, and rematerialising a
  constant instead of spilling it.
- **A register-poor test target**: run the callgrind-based Ir measurement with a simulated pool of 6 registers (a `--regs=N` flag on the
  allocator) so every perf PR reports the register-poor number next to the normal one (standing rule 2026-11-07). That flag is the first
  thing to add, because it makes the claim measurable.

## Risks

- Items 1 and 7 change what debuggers and tools see; keep `--symbols` and test that `pelago build --symbols` still lists every function.
- Item 4 changes the stop message path; the existing tests for "stack overflow" must keep their output text.
- Item 6 can slow `fib`/`tak` if a hot function loses its alignment; keep the alignment for functions marked hot and rerun those two rows.
- Item 7 needs a fixed-point loop in the emitter; AArch64 has fixed-size instructions and is unaffected.

## Questions for the owner

1. Is `.symtab` by default off (item 1) acceptable? It stays available behind a flag.
2. Should `hello`-class programs get a smaller default output buffer (item 5), or keep 64 KiB on desktop targets and a small one only for a
   `--target=embedded` later?
3. Do you want `--opt-size` as a flag (item 12), or a size-first default for non-x86 targets only?
4. May the optimiser drop the 48-byte function alignment for non-hot functions (item 6), accepting a small timing change on rows like `fib`?
5. Order: I suggest 1, 2, 3, 6 and 8 first (small, no codegen risk), then 4 and 5, then 7, then the heap (11).
