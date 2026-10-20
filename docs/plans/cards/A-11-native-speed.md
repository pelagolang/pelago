Id: A-11
Title: Native speed: from 4.5 times slower than C at -O2 towards 1.5 to 2 times
Column: Doing
Work items: [a11-work-items.md](../a11-work-items.md) (the open work, split for parallel threads)
Order: after A-09 N6 (track A)
Depends on: A-09; A-04 (the IR optimiser)
Size: XL, in stages (S1 to S6), each measured
Touches: `compiler/src/native/` (instruction selection, register assignment), `compiler/src/ir/opt/` (new passes), `docs/design/backend.md`

# Where we are

On the collatz-and-`fib(35)` program: interpreter about 7 s, native with registers and the optimiser 0.45 s, C `-O0` 0.23 s, C `-O2` 0.10 s (`docs/design/backend.md`, section 6).
We are about twice C at `-O0` and 4.5 times C at `-O2`. Two groups of causes: what the *language* asks for, and what our *backend* does badly.

# What costs time, biggest first

| # | Cause | Kind | What fixes it |
|---|---|---|---|
| 1 | **Division and remainder by constants.** `x % 2`, `x / 2` run `idiv` (20 to 40 cycles) plus our zero and `-1` checks. C turns them into a shift or a multiply. | backend + optimiser | S1: the optimiser rewrites `x / 2^k`, `x % 2^k`, and division by other constants into shifts/multiplies (the "magic number" method); signed forms need the usual correction. Constant divisor also removes the zero and `-1` checks. |
| 2 | **Checked arithmetic.** Every `+ - *` is followed by `jo`/`jc`. C has no checks. | language decision | S2: range analysis in the optimiser (a loop counter bounded by its compare cannot overflow; `i + 1` after `i < n`), merging checks, moving them out of loops. Keep the checks that matter; `+%` etc. stay free. This is the cost of safe arithmetic and should not be removed wholesale. |
| 3 | **Values in memory.** Only five callee-saved registers, one per value for its whole life, no splitting; every other value costs a load and a store per use; every operation is `ld rax; ld rcx; op; st`. | backend | S3: real linear-scan register allocation with live-range splitting, all registers (caller-saved ones for values not live across calls), two-address instructions working directly on registers, compare-and-branch fused (`cmp` + `jcc`, no `setcc`/`test`), addressing modes (`[base + index*8 + disp]`), constants as immediates. This needs a machine-level IR between the IR and the bytes (see A-10, option 2). |
| 4 | **Calls.** Frame setup, saving callee-saved registers, passing everything through memory, no inlining of small functions unless `--opt` is on. | backend + optimiser | S4: `build` uses the optimiser by default; inlining thresholds tuned on the benches; leaf functions without a frame pointer; arguments stay in registers. |
| 5 | **Slots zeroed on every execution** (an interpreter rule), reused memory zeroed on `alloc`. | backend + IR | S5: drop the rule where the lowering writes before reading (the verifier or lowering can say which slots need it); promote slots to values (A-04 already does much of this, extend it); inline the allocator's fast path. |
| 6 | **Small copies and fills** use `rep movsb`/`rep stosb` (a fixed start-up cost of ten or more cycles). | backend | S5: constant sizes up to 64 bytes become a few `mov`s. |
| 7 | **Loops**: no invariant hoisting, no unrolling, no alignment of loop heads. | optimiser | S6: loop-invariant code motion, strength reduction (`i * 8` to an add), loop alignment, branch layout (likely path falls through). |

# Method

Measure first, change one thing, measure again. `compiler/benches/` gets a few more programs (the collatz/`fib` pair, a list sum, a string builder, a map workload, a struct-heavy loop, a recursion test) and a script prints the table "interpreter / native / C -O0 / C -O2" with the same algorithm in C. Every change is covered by the existing three-way tests, plus new tests for each optimiser pass (before and after must print the same, with the edge cases of `backend.md` section 2).

# Expectations (honest)

S1 alone should remove a big share of the collatz time. S1 to S3 together should reach about 1.5 to 2 times C `-O2` on integer code; matching C needs inlining heuristics, vectorisation and more, which belong to ROADMAP Phase 6. Float-heavy code is another matter: today every float lives in memory, so it gains the most from S3.

# Questions for the owner

1. Order S1 (constants), S2 (checks), S3 (registers)? Recommended: yes, S1 first because it is small and visible.
2. Keep overflow checks on in release builds (recommended; a flag may turn them off later, with the checks themselves optimised first)?
3. Introduce the machine-level IR with S3 (recommended)?

# Progress

**S1, part 1 (done): powers of two.** `ir/opt/divconst.rs` rewrites `x / 2^k` and `x % 2^k` (signed and unsigned) into shifts and masks, with the usual bias for negative dividends, and `x % 2^k == 0` (or `!= 0`) into `x & (2^k - 1)` compared with zero. Tests compare before and after for `i8`, `i32` and `i64` at the edges. On `compiler/benches/collatz.pel` (steps for 1 to 300,000): native `--opt` 0.30 s before, 0.23 s after; C `-O0` 0.15 s, C `-O2` 0.07 s on this container.
**S1, part 2 (done): other constant divisors.** New IR operations `MulHi`/`UMulHi` (the high half of the full product, integers up to 64 bits) in the IR, verifier, interpreter and x86 generator (`imul`/`mul` one-operand form, or a 64-bit multiply and a shift for narrow types). `divconst.rs` rewrites `x / c` and `x % c` for any constant `c` with the magic numbers of Granlund and Montgomery (unsigned, with the add-and-shift form when the multiplier needs one more bit) and Hacker's Delight (signed); a remainder is `x - (x / c) * c`. Left alone: 0, 1, -1 and negative powers of two (`MIN / -1` still wraps, zero still stops). Tests: the magic plans are checked exhaustively for 8 bits and at the edges for 16, 32 and 64; whole-IR differential tests for `i8` to `i64`; native three-way tests with loops. New bench `compiler/benches/divmod.pel` (digit sums, `% 3`, `/ 7` over 3,000,000 numbers): native `--opt` 0.45 s before, 0.086 s after; C `-O2` 0.041 s.

**S3, first round (done): the generator stops wasting moves.** Constants are immediates (no register, no frame slot); a comparison that only feeds the branch ending its block is not stored (`cmp` + `jcc`), and a jump to the next block falls through; operands are used from their own registers and results are built in place; `r10` and `r11` hold values that are not live across a call. Still open from S3: real linear scan with live-range splitting, all caller-saved registers, addressing modes, a machine-level IR (needed for A-10 option 2).
**S2, first round (done): checks that cannot fire.** `ir/opt/ranges.rs` finds values known to be small and non-negative (a widened byte, `x & 255`, `x % 10`, `x >> 3`, sums and products of those) and turns a checked `add`, `sub` or `mul` that provably fits into the plain operation. It does not look through loops: block parameters and loads have no range, so a loop counter's `+ 1` keeps its check unless the lowering already emitted a plain add (as it does for `for i in 0..n`). Next for S2: induction variables (a counter that starts at a constant, goes up by 1 and is bounded by a dominating compare).

**S2, second round (done): loops.** `ir/opt/ranges.rs` now gives block parameters the join of what their edges pass (repeated to a fixed point, widening after a few steps), and cuts a range with the comparison that guards the block: a block entered only by one side of `branch (lt i, n)` knows `i < n` in everything it dominates (`ir/opt/dom.rs`). So `while i < n { i += 1 }` loses its check; `i += 2`, `i <= n`, and a counter nothing bounds keep theirs. Signed counters only for now: an unsigned `i < n` with unknown `n` bounds `i` by `u64::MAX`, which the shared signed/unsigned range cannot hold. Across `examples/programs` and the benches the optimised IR has 407 checked operations instead of 493.

**Loops (W9, done in part): `ir/opt/licm.rs`, `ir/opt/layout.rs`, `ir/opt/loops.rs`.** Loop-invariant code motion moves arithmetic, compares, casts and address arithmetic that cannot stop in front of the loop (a load too when the loop writes no memory and the load is in the header or reads a slot of the function; division never, since the loop may run zero times). Layout puts a loop's body first and its header last, so an iteration ends in one conditional jump back instead of a conditional jump out plus a jump back, and moves blocks that only `stop` to the end. Measured with `compare.sh` (best of 7): loop_sum 0.68 s to 0.39 s, sieve 0.26 s to 0.19 s, mapwork 0.053 s to 0.043 s, the rest within noise. Not done: strength reduction (`i * 8` into an add) because it fights the addressing modes planned in W5, which absorb the scale; aligning loop heads needs `Asm::align` in the generator (collatz moves between 0.12 s and 0.14 s when unrelated code shifts it, so this is worth doing).

Measured with `compiler/benches/compare.sh` (best of 7, this container):

| Program | before (S1) | now | C -O0 | C -O2 |
|---|---|---|---|---|
| collatz, 1 to 300,000 | 0.23 s | 0.09 s | 0.14 s | 0.07 s |
| fib(38) | 0.36 s | 0.30 s | 0.34 s | 0.08 s |

**S4, first part (done): `build` optimises by default.** `pelago build` and `pelago run --native` run the optimiser unless `--no-opt` is given (`--opt` is still accepted and does nothing there); `pelago run` and `pelago ir` stay unoptimised unless `--opt`. The inliner threshold (`SMALL` in `ir/opt/inline.rs`) was tried at 15, 40 and 100 on the ten benches: all differences were within run-to-run noise, so it stays at 40. Leaf functions without a frame pointer and arguments kept in registers are generator work (`native/gen.rs`) and wait for the backend thread.

PRs: #7 (S1, powers of two), #8 (S2 and S3, first rounds). The open work, with files and dependencies for each item, is in [a11-work-items.md](../a11-work-items.md).
