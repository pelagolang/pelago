Id: A-09
Title: Native backend: x86-64 machine code and ELF straight from the IR
Column: Done (N1 to N6 and the Windows target; the leftovers are in: buffered `print`, `float32` rounding, saturating float to int, stack and bad-address guard. `--emit asm` and N7 are separate cards)
Order: 1 (track A)
Depends on: nothing open (the IR, the interpreter and the optimiser exist)
Size: XL, in seven stages (N1 to N7), each testable on its own
Touches: new `compiler/src/native/`, `main.rs` (`pelago build`), tests, `docs/design/backend.md` (to be written at N1), `ROADMAP.md` Phase 3

# Decisions (the owner answered "yes" to the recommendations)

1. First target **x86-64 Linux, static ELF64**. **Windows (PE) follows right after N3**, tested by a `windows-latest` job in CI (the owner runs Windows 11 without WSL, so a Windows executable matters).
2. **No linker, no libc** in version 1: our own ELF writer; direct system calls.
3. **Simple code generator first**, then linear-scan register allocation (N5).
4. **Our own System V-shaped calling convention.** (On Windows the calls into `kernel32` use Microsoft's convention; ours stays internal.)
5. **Emitted size-class allocator** now; a library allocator in Phase 4.
6. **`stopped: msg` on stderr, exit status 1** for a stop, like the interpreter.
7. **Start now with N1.**

Also decided: the owner wants `main` left alone while coding (work goes on feature branches); the editor work (S-05, S-06) is built and in Review.

# Why

Everything that is checked is lowered and runs in the IR interpreter. The project promise is "compiled to native code by our own backend, no interpreter, no VM, no
runtime" (ROADMAP). Until this card is done, programs run about two orders of magnitude slower than they will, benchmarks against C and Rust are impossible, and
the shelf items that need real machine features (S-01 measured maps, S-02 threads, S-04 async, A-06 blocked `soa` layout) cannot be measured or built.

# What the backend has to deal with (read from the code as it stands now)

- **A small IR.** Types: `Int(8|16|32|64)`, `F32`, `F64`, `Bool`, `Ptr`, `Void`. 22 operations: `Const`, `Bin` (add sub mul div rem udiv urem and or xor shl shr ushr),
  `Checked` (add/sub/mul that stop on overflow), `Cmp` (signed and unsigned orderings, floats), `Neg`, `Not`, `Cast` (zext sext trunc int-to-float float-to-int float
  resize ptr-to-int int-to-ptr), `Call`, `CallPtr`, `FuncAddr`, `DataAddr`, `Slot`, `PtrAdd`, `Load`, `Store`, `MemCopy`, `Alloc`, `Free`, `Print`. Terminators: `Jump`,
  `Branch`, `Return`, `Stop`, `Unreachable`. SSA with block parameters (no phi nodes). No unwinding.
- **Everything else is library code.** Strings, lists, maps, sets, closures, tasks, channels, `box`, `soa`, `meta` results are all written in `compiler/core/core.pel`
  over those 22 operations. The environment the backend must provide is only: **write bytes to the output, allocate and free memory, stop the program.**
- **The interpreter is the oracle.** Every lowering test already runs the program twice (with and without `--opt`) and compares. The native build becomes a third
  column of the same tests.
- **Machine tools in this container** (`as`, `ld`, `objdump`, `gdb`, `ndisasm`, `gcc`, x86-64 host) are used only as *oracles in tests*, never as part of the compiler.

# Semantics to pin down before any code (the interpreter decides today)

| Case | Interpreter | Native must |
|---|---|---|
| division or remainder by zero | stops "division by zero" | test and branch to the stop block (the CPU would trap with SIGFPE) |
| signed `MIN / -1` | wraps (`wrapping_div`) | guard it: x86 `idiv` traps on it |
| shift by the width or more | stops "shifted by the width or more" | test and branch (x86 masks the count silently) |
| checked add/sub/mul overflow | stops | `add` plus `jo` (and `jc` for unsigned) to a shared stop block |
| float to int | (check `FloatToInt` in `interp.rs`) | same result, including NaN and out of range; write a table test first |
| float compares with NaN | as Rust | `ucomisd` flags: `Eq` false, `Ne` true, ordered compares false |
| `Print` | appends to a buffer | `write(1, ptr, len)`; buffer later (N6) |
| `Stop(msg)` | `stopped: msg`, exit status 1 | write `stopped: msg` to fd 2, `exit_group(1)` |
| leak count at exit | reported | a test-build flag makes the allocator count live blocks; report the same way |

# Design (recommendations; the numbered questions at the end are for the owner)

1. **First target: x86-64, Linux, static ELF64 executable.** No libc. Direct system calls: `write`, `exit_group`, `mmap`, `munmap`. Reason: the dev container, CI and
   every test machine here are x86-64 Linux; the owner's Windows laptop runs the result through WSL. Windows (PE) and macOS (Mach-O) come after, as new writers over the
   same machine code.
2. **No linker, no object files in version 1.** The compiler already sees the whole program (monomorphised, dead code removed), so it lays out one image directly: our
   own ELF writer. "Linking story" in ROADMAP Phase 3 becomes: none until there is a C FFI or separate compilation.
3. **Our own x86-64 encoder** (`compiler/src/native/x86.rs`): only the forms the code generator uses (mov, movzx/movsx, lea, add/sub/imul/idiv/div, and/or/xor/not/neg,
   shl/shr/sar by cl, cmp/test, setcc, jcc/jmp/call/ret, call through register, push/pop, `rep movsb`, SSE2 scalar float: movss/movsd, add/sub/mul/div, ucomis, cvtsi2s,
   cvtts2si, cvtss2sd/cvtsd2ss, xorps). Tested byte for byte against golden output of `as` (golden bytes stored in the tests, so CI does not need `as`).
4. **Two code generators in sequence, one after the other, same tests.**
   - **Simple (N2):** every SSA value lives in its own stack slot; each IR operation is a fixed template using `rax rcx rdx` and `xmm0 xmm1`. Slow (like -O0) but
     small, obviously correct, and enough to run the whole test suite natively.
   - **Fast (N5):** linear-scan register allocation over the *optimised* IR (A-04, switched on by default for builds), block parameters resolved as parallel
     copies, callee-saved registers for values live across calls. It is a pure speed change, protected by the same tests.
5. **Calling convention: ours, shaped like System V.** Integer and pointer arguments in `rdi rsi rdx rcx r8 r9`, floats in `xmm0` to `xmm7`, the rest on the stack, result in
   `rax` or `xmm0`, callee-saved `rbx rbp r12-r15`, stack 16-byte aligned at calls. (`sret` is already an explicit pointer parameter in the IR.) Using the
   System V shape now means a future C FFI and debuggers work without a second convention.
6. **Runtime, emitted by the backend (tiny):** `_start` (calls the entry function, then `exit_group(0)`); a shared `stop` routine; `print` as one `write` call; an allocator.
   The allocator is size-class free lists over `mmap`'d chunks, with an 8-byte header holding the class (the IR's `Free` carries no size). Phase 4 may replace it
   by a library written in Pelago over a `Syscall` operation, which fits the "library over a few intrinsics" decision IR6.
7. **No debug information in version 1**, but the span of each IR instruction is kept in a side table so N7 can emit a DWARF line table and `gdb` can show source lines.
8. **Data.** String literals and tables go into a read-only section, mutable tables (object tables, once-flags: S-03 item 2) into a writable one; `DataAddr` becomes a
   RIP-relative `lea`. (Baked `meta` data is built by code today and stays that way.)
9. **Compile-time code stays in the interpreter** (`meta` must run during compilation, so the interpreter remains in the compiler). The A-05 risk applies: two engines
   that must agree. N3 adds "same result at compile time and run time" tests.

# Stages

| Stage | Work | Done when |
|---|---|---|
| **N1** (done) | `native/x86.rs` encoder, `native/elf.rs` writer, runtime stub. Hand-built IR modules only. | A hand-built module prints `hello` and exits 0 as a real ELF; encoder bytes match golden `as` output; `objdump -d` reads the file. |
| **N2** (done) | Simple code generator for all 22 operations and the 5 terminators (including the semantics table above). `pelago build file.pel -o out`. | Hello world, FizzBuzz, `fib` and `loop_sum` run natively and print what the interpreter prints. |
| **N3** (done) | The whole suite natively: every `output_of` test and `example_programs_run` also run the native build (Linux x86-64 only, skipped elsewhere) and compare output and exit status; allocator leak counter in test builds; compile-time vs run-time agreement tests. | The 368 tests are green in all three modes (interpreter, optimised, native). |
| **N4** (mostly: `build`, `run --native`; `--emit asm` left) | CLI: `pelago build`, `pelago run --native`, `--emit asm` (a listing for reading). Docs: `docs/design/backend.md`. | A user can build and run an example from the command line. |
| **N5** (done, a simpler scheme than linear scan: callee-saved registers for the heaviest non-overlapping intervals) | Linear-scan register allocation; `--opt` on by default for `build`. | All tests green; `benches/*.pel` at least 5x faster than N2; numbers recorded in the card. |
| **N6** | Speed work found by measuring: buffered `print`, inline `MemCopy` for small sizes, shared stop blocks, branch layout. | Benchmarks recorded; a table "ours vs the interpreter vs C (gcc -O2 on the same program)" in `docs/design/backend.md`. |
| **W** (done) | Windows PE target: `native/pe.rs`, a Windows runtime, `--target=windows`, a `windows-latest` CI job, `PELAGO_WINE=1` for local runs under Wine. | All tests green in the Windows column (Wine) and the CI job. |
| **N7** | Later, not part of the first milestone: DWARF line table, real threads (S-02: `clone` and atomics), AArch64, PE and Mach-O writers. | Each its own card. |

# Tests

- Encoder: one test per instruction form against golden bytes from `as` (a script in `compiler/tests/` regenerates them when `as` is present).
- Differential: native versus interpreter on every lowering test, the examples and the benches; the same exit status for a `stop`.
- Edge cases from the semantics table: zero division, `MIN / -1`, shifts by 0, 1, width-1, width, NaN compares, float to int at the extremes, checked overflow of each width.
- Leaks: the test build's allocator counts live blocks at exit.
- Robustness: a 200000-deep recursion (the interpreter limit today) either works natively or stops cleanly with a stack message; set an explicit 64 MB main stack with `mmap` plus
  a guard page.

# Risks

- **Two engines must agree** (interpreter for `meta`, native for programs). Mitigation: shared semantics table, N3 tests.
- **A code generator bug looks like a language bug.** Mitigation: the simple generator exists so a wrong result from N5 can be compared with N2 and with the interpreter.
- **Effort.** N1 to N4 is the milestone "hello world is a native binary" (ROADMAP Phase 3). N5 is where the speed comes from. Do not start N5 before N3 is green.
- **Windows owner.** The owner's machine is Windows; native output is Linux ELF until a PE writer exists. WSL covers it in the meantime. Say so in the README when N4 lands.

# Questions for the owner (answered, see Decisions)

1. First target x86-64 Linux ELF (recommended), or Windows PE first because that is your machine?
2. No linker and no libc in version 1, our own ELF writer (recommended), or emit object files for the system linker?
3. Simple code generator first, then register allocation (recommended), or straight to register allocation?
4. Our own System-V-shaped calling convention (recommended)?
5. Emitted size-class allocator now, a library allocator in Phase 4 (recommended)?
6. `stopped: msg` on stderr and exit status 1 for a stop, like the interpreter (recommended)?
7. Start now with N1, or keep this as a plan until you say go?

# What is left from A-09 (decided approach for each; to be done as N6)

| Item | Approach |
|---|---|
| **Buffered `print`** | A 64 KiB buffer in `.data`; `Print` copies into it and flushes when full; flushed by `rt_exit`, by `rt_stop` (before the message, so the order of standard output and standard error stays right) and before the program returns from `main`. One `write` per 64 KiB instead of one per `print`. |
| **`--emit asm`** | **Not built; replaced** by reading the output with the tools that exist: the ELF has section headers and function symbols, so `objdump -d out` and `gdb` show our code by function name; for PE use `objdump -D -b pei-x86-64`. Documented in `backend.md`. We do not write a disassembler. |
| **The optimiser by default** | `pelago build` optimises unless `--no-opt` (every test already runs plain and optimised); `run` stays as it is until check removal (A-11 S2) is done. |
| **`float32` rounding** | Make the interpreter round after every `float32` operation and conversion (`as f32 as f64`) so both engines agree; add tests with values that are not exactly representable. |
| **`FloatToInt` out of range** | Define it as in Rust's `as`: **saturate** to the target width, NaN becomes 0. The interpreter changes to match; natively, `cvttsd2si` plus compares against the bounds. |
| **Stack overflow and bad addresses** | The interpreter reports precise memory errors; native code cannot (it is not memory-checked). A guard page below the big stack plus a fault handler (Linux: `sigaltstack` and `rt_sigaction`; Windows: a vectored exception handler) turns a crash into `stopped: stack overflow` or `stopped: invalid memory access` and exit status 1. Documented as a difference. |
