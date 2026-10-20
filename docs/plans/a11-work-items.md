# A-11 work items (for parallel work)

Status after PRs #7 to #12 and the register allocator (W3) (card [A-11](cards/A-11-native-speed.md) has the longer story). Each item below can be a thread of its own: it says what to do, which files it touches, what it needs first, and which other items it will collide with in git. Branch names follow `claude/<type>/<name>`.

## Where we are

Measured with `compiler/benches/compare.sh` (best of 7, one container, native `--opt`):

| Program | Start of A-11 | Now | C -O0 | C -O2 |
|---|---|---|---|---|
| collatz, 1 to 300,000 | 0.30 s | 0.09 s | 0.14 s | 0.07 s |
| fib(38) | 0.36 s | 0.30 s | 0.34 s | 0.08 s |

Done:

- **S1 (PR #7):** `ir/opt/divconst.rs` turns `x / 2^k`, `x % 2^k` (signed and unsigned) into shifts and masks, and `x % 2^k == 0` into one `and`.
- **S2, first round (PR #8):** `ir/opt/ranges.rs` drops a checked `add`, `sub`, `mul` when both operands have a known small non-negative range (widened bytes, `x & 255`, `x % 10`, `x >> 3`, sums and products of those). Loops are not analysed.
- **W2 (PR #11):** `ir/opt/ranges.rs` now follows block parameters to a fixed point (with widening) and cuts ranges by the comparison that guards a block, using the new `ir/opt/dom.rs`. `while i < n { i += 1 }` and counting down while `i > 0` lose their checks; signed counters only.
- **W9 (PR #16), except alignment and strength reduction:** `ir/opt/loops.rs`, `ir/opt/licm.rs` (invariant code motion) and `ir/opt/layout.rs` (loop test at the bottom, `stop` blocks last). Strength reduction was left out on purpose: it fights the addressing modes of W5. Loop-head alignment (`Asm::align` in `gen.rs`) is still open.
- **S3, first round (PR #8):** `native/gen.rs` keeps integer constants as immediates (`Where::Imm`), fuses a compare with the branch that ends its block, falls through to the next block, uses operands from their own registers and builds results in place, and gives `r10`/`r11` to values not live across a call (`native/regs.rs`).

- **W3 (PR #15):** the allocator in `native/regs.rs` (live ranges with holes, all the caller-saved registers, split around calls, a cost model, hints) and parallel moves in `gen.rs`; see `docs/design/backend.md` section 6.

- **W5:** loads and stores use `[base + index*scale + disp]` and frame offsets, small `memcopy` is plain moves, a `Slot` is not cleared when the code after it overwrites it; see `docs/design/backend.md` section 6.

Still true: the generator is template based (`rax`, `rcx`, `rdx` scratch), a value that gets no register costs a load and a store per use, floats always live in memory, and structs live in memory (the optimiser does not split them into fields yet).

## Rules the generator now relies on (read before touching `gen.rs` or `regs.rs`)

1. **A result may share a register with an operand that dies in the same instruction.** The allocator's ranges are half open and an instruction writes its result just after it reads its operands. Any template that writes the result's register before it has read every operand is wrong: `bin` and `checked` handle it (the result in `b`'s register swaps a commutative operation and builds `sub` in `rax`); the other templates build in `rax` and store last. Keep it that way.
2. **Constants have no storage.** `const_values()` marks them `Where::Imm`; `ld`/`ld_ext`/`imm32` read them, `st` panics on them. `regs.rs` skips them. Float constants are not immediates (a float constant is built in its xmm register or frame slot). A value nothing reads is `Where::Dead` (writes are dropped, reads panic).
3. **Registers.** `rax rcx rdx` are the templates' scratch (and the go-between of `Gen::moves`); `r10 r11 r8 r9 rsi rdi` hold values that are not live across a call-like op (`Call`, `CallPtr`, `Alloc`, `Free`, `Print`, `MemCopy`), or are split around it (stored before, loaded after, `Alloc::across`); `rbx r12 to r15` survive calls and are saved in the prologue. Anything that needs particular registers (call arguments, entry parameters, `memcopy`, `print`) must go through `Gen::moves`, never a sequence of `ld`s. A template that uses `rdi`, `rsi`, `r8` to `r11` itself (the large `Slot` clears with `rep stosb`) must save it.
4. **Edges:** `edge_copies()` lists what a jump must copy (nothing for a parameter that is dead or already in place), `moves()` makes the copies at once, `edge()` jumps unless the target is the next block. A branch with copies puts the side it does not fall into out of line (`stub()`).
5. **Canonical form:** integers and booleans in registers or slots are zero-extended to 64 bits; signed uses widen with `ld_ext`.
6. **Differential tests are the safety net:** `lower/tests.rs` (interpreter against native, plain and optimised, Linux; Windows through the CI job) and `ir/opt/tests.rs` (before and after each pass). New passes and new instruction forms need cases in both.

## Work items

Size is S (hours), M (a day), L (several days). "Collides with" means the same function or file will probably conflict if two threads run at once; run those one after the other or in one thread.

### W1. High multiply and constant division by any divisor (A-11 S1 part 2) (done)
- **What:** add `MulHi`/`UMulHi` to the IR, then rewrite `x / c` and `x % c` for any constant `c` with the magic-number method (signed needs the correction step). `x / 10` is the common case.
- **Touches:** `compiler/src/ir/mod.rs` (`BinOp`), `ir/verify.rs`, `ir/interp.rs` (`bin`), `ir/print.rs`, `ir/opt/divconst.rs`, `native/gen.rs` (`bin`: `mul`/`imul` one-operand form, `native/x86.rs` already has `mul`), `native/x86_golden.rs` if a new encoding is added, `ir/opt/tests.rs`, `native/tests.rs`.
- **Depends on:** nothing. **Size:** M.
- **Collides with:** W3, W5, W8 in `native/gen.rs` (small hunk in `bin`), W8 in `ir/interp.rs`.
- **Check:** exhaustive-at-the-edges tests for `i8`/`i32`/`i64` signed and unsigned, divisors 3, 5, 7, 10, 100, 641, `MAX`, `MIN+1`; collatz-style bench with `% 3`.

### W2. Induction variables for the overflow checks (A-11 S2 part 2) — done, PR #11
- **What:** a loop counter that starts at a constant, goes up by a constant each iteration and is bounded by a dominating compare (`i < n`) cannot overflow at its `i + 1`. Needs dominators and ranges for block parameters (a fixed point with widening).
- **Touches:** `ir/opt/ranges.rs`, new `ir/opt/dom.rs` (dominator tree; W9 reuses it), `ir/opt/mod.rs` (one line), `ir/opt/tests.rs`.
- **Depends on:** nothing. **Size:** M.
- **Collides with:** nothing in the backend; W9 wants the same dominator code (do W2 first or share `dom.rs`).
- **Check:** `for i in 0..n` with `s += i` style sums keep or lose exactly the checks the proof allows; the unprovable ones (`s += 1` with no bound) stay; tests on hand-written IR with a loop, plus differential runs.

### W3. Real register allocation (A-11 S3 part 2) — done
- **What:** replace the one-interval greedy scheme with linear scan: live ranges with holes, spilling one range at a time (split instead of whole-value memory), the caller-saved registers `rsi rdi r8 r9` when no call is live, and callee-saved ones otherwise. Handle the entry parameters and call arguments with a parallel move.
- **Touches:** `native/regs.rs` (rewrite), `native/gen.rs` (`layout`, `ld`/`st` paths, `edge`, `call`, prologue), `native/tests.rs`.
- **Depends on:** nothing, but read the rules above. **Size:** L.
- **Collides with:** W4, W5, W6, W11 (all in `gen.rs`) and W1 (small). Run W3, W5, W11, W4 in one thread, in that order.
- **Check:** `--no-regalloc` still works; three-way tests green; `compare.sh`: fib(38) should drop clearly (today most of its time is call setup and spills).

### W4. A machine-level IR between the IR and the bytes (needed by A-10 option 2)
- **What:** virtual registers and two-address instructions with one instruction selector per CPU; the allocator of W3 then works on machine code.
- **Touches:** new `native/mir.rs`, `native/gen.rs` (becomes the selector), `native/regs.rs`.
- **Depends on:** W3 (do the allocator first against the current generator, then lift it). **Size:** L.
- **Collides with:** every other `native/` item. Only start it when W3 and W5 are merged.
- **Why:** card A-10 (AArch64) should share this layer rather than copy `gen.rs`; decision recorded in A-10.

### W5. Cheaper memory operations — done (addressing modes, frame objects, small copies, slot clearing)
- **What:** addressing modes (`[base + index*8 + disp]`) for `PtrAdd` + `Load`/`Store`; stop zeroing a `Slot` that the code writes before reading; constant-size `MemCopy` up to 64 bytes as plain moves; zero-fill without `rep stosb` for small slots.
- **Touches:** `native/gen.rs` (`Op::Slot`, `MemCopy`, `Load`, `Store`, `PtrAdd`), `native/x86.rs` (a base+index memory operand and its golden tests in `x86_golden.rs`), possibly `lower/` where a slot is declared; `ir/opt/promote.rs` if "written before read" is proven in the optimiser instead.
- **Depends on:** nothing. **Size:** M.
- **Collides with:** W3, W11, W4 (`gen.rs`); W1 slightly.

### W6. Calls and the default pipeline
- **What:** `pelago build` optimises by default (`--no-opt` to turn off); leaf functions without a frame pointer; keep arguments in registers across the prologue; tune the inliner thresholds on the benches.
- **Touches:** `compiler/src/main.rs`, `native/gen.rs` (`function`, `epilogue`, `call`), `ir/opt/inline.rs`, docs (`README.md`/`CLAUDE.md` mention `--opt`).
- **Depends on:** W7 (benches to tune on). **Size:** M.
- **Collides with:** W3, W5 (`gen.rs`).

### W7. More benchmarks and a regression table
- **What:** add programs with a C twin in `compiler/benches/c/`: list sum, string builder, map workload, struct-heavy loop, float loop (n-body style), recursion with several arguments, a sieve. Extend `compare.sh` to print one table and optionally a JSON line per program for a CI artifact. The weekly benchmark routine can reuse it.
- **Touches:** `compiler/benches/*.pel`, `compiler/benches/c/*.c`, `compiler/benches/compare.sh`, `docs/design/backend.md` (the table).
- **Depends on:** nothing. **Size:** S to M. **Collides with:** nothing. Do this first; everything else measures against it.

### W8. What is left from A-09 (its card has the decided approach for each)
- **What:** buffered `print` (64 KiB buffer, flushed at exit, stop and return from `main`); `float32` rounding after every operation in the interpreter; `FloatToInt` saturating like Rust's `as` (NaN becomes 0) in both engines; a guard page and fault handler turning stack overflow and bad addresses into `stopped: ...` with exit status 1 on Linux and Windows.
- **Touches:** `native/rt.rs`, `native/elf.rs` and `native/pe.rs` (guard page, handler), `ir/interp.rs` (`cast`, float ops), `native/gen.rs` (`cast`, `Print`), `native/tests.rs`, `lower/tests.rs`, `docs/design/backend.md`.
- **Depends on:** nothing. **Size:** M. It splits into two threads if wanted: (a) runtime and print buffering, (b) float semantics in interpreter and `cast`.
- **Collides with:** W1 (`interp.rs`), W3/W5 (`gen.rs`, small hunks).

### W9. Loop optimisations — done in PR #16 except alignment (open) and strength reduction (dropped)
- **What:** loop-invariant code motion, strength reduction (`i * 8` into an add), alignment of loop heads, and branch layout (likely path falls through).
- **Touches:** new `ir/opt/licm.rs`, `ir/opt/mod.rs`, `ir/opt/tests.rs`, `native/gen.rs` (alignment only, via a new `Asm::align`).
- **Depends on:** W2 (dominators). **Size:** M to L. **Collides with:** `gen.rs` only for alignment.

### W11. Floats in registers (done: PR `claude/perf/float-regs`)
- **What:** keep float values in `xmm` registers (callee-saved does not exist in the System V ABI, so only for ranges without calls); arithmetic directly on registers; float compare fused with branches.
- **Touches:** `native/regs.rs`, `native/gen.rs` (`bin`, `cmp`, `cast`, `call`), `native/x86.rs`.
- **Depends on:** W3 (the allocator must handle two register classes). **Size:** M to L. **Collides with:** the `gen.rs` group.

### W10. AArch64 (card A-10)
- Starts after W4. Not part of A-11; listed so nobody starts it early.

## Suggested threads

| Thread | Items | Branch names |
|---|---|---|
| Bench first | W7, then W6 | `claude/chore/bench-suite`, `claude/perf/default-opt` |
| Backend core (serial) | W3, W5, W11, W4 | `claude/perf/linear-scan`, `claude/perf/memory-ops`, ... |
| Arithmetic | W1 | `claude/perf/mulhi-division` |
| Checks and loops | W2, then W9 | `claude/perf/induction-ranges`, `claude/perf/licm` |
| Leftovers | W8 (a), W8 (b) | `claude/feat/print-buffer`, `claude/bug/float-semantics` |

Merge order matters only inside the backend group. Every thread should rebase or merge `main` before opening its PR, run `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test`, and quote `compare.sh` numbers in the PR.
