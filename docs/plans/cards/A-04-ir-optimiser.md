Id: A-04
Title: IR optimiser: slots to registers, drop flags away, inlining
Column: Done for stages 1 to 7 (default-on, bounds-check removal and the extras of section 7 remain)
Order: 4
Depends on: A-03 (not strictly; the ledger removes many flags first, the optimiser removes the rest)
Size: XL (do it in the stages below; each stage ships on its own)
Touches: `compiler/src/ir/` (new `opt/` folder), `pelago` (`--opt` flag), tests, `docs/design/ir.md`

# 1. Why now

The lowering is deliberately simple: every variable lives in a **stack slot**, every owned value has a **drop flag** stored in a slot, and small library
functions (`rt_hdr_len`, `rt_hdr_data`) are real calls. That made the lowering easy to get right, and the interpreter proves it is right.
For a machine-code backend it is far too slow: every `var i` is a memory load and store, every owner pays flag traffic, every list access is a call.
The IR is an SSA form with block parameters, built for exactly the passes below, and we are working in the IR now, so this is the right time. The performance goal of the
language ("no hidden cost") makes this the single biggest speed item on the roadmap.

The best property of the situation: **we can test every pass for free.** The interpreter runs every program before and after a pass, and the results (output, result value, leaks)
must be identical. That is *differential testing*, and the suite already has about seventy programs.

# 2. The pipeline

Run per function, in this order, repeating the cheap clean-ups until nothing changes (a fixpoint, capped at a few rounds):

| # | Pass | What it does | Why it matters |
|---|---|---|---|
| 1 | **Simplify CFG** | Remove unreachable blocks (`Func::compact` exists), merge a block into its only predecessor, thread jumps through empty blocks, fold a branch whose two targets are equal. | Cleans up after every other pass. |
| 2 | **Promote slots** (mem2reg) | A stack slot whose address is never used except by `Load` and `Store` of one type becomes an SSA value. Block parameters carry it across control flow. | Variables become registers. |
| 3 | **Split slots** (SROA) | A slot only touched through constant-offset `PtrAdd` + `Load`/`Store` is split into one slot per field, then promoted by pass 2. | Small structs, tuples, `option<int>`, `string` headers live in registers. |
| 4 | **Constant folding and propagation** (SCCP) | Evaluate what is known at compile time across blocks; mark unreachable branches. | Turns `if flag` into a jump when the flag is a known constant. |
| 5 | **Copy propagation, dead code elimination** | Remove values nobody reads and stores nobody reads (stores to slots that no longer exist). | Removes the leftover flag stores. |
| 6 | **Inline small functions** | Replace a call with the callee body when the callee is small, has one call site, or is marked `@inline`. Run again after inlining. | `rt_hdr_len`, `rt_hdr_data`, `[int].len`, glue wrappers disappear. |
| 7 | **Redundant check removal** (later) | Drop bounds and overflow checks the facts prove unnecessary (`for i of 0..n` indexes with `i < n`). | A later stage, not required for the milestone. |

## 2.1 Promote slots, concretely

Our IR already uses block parameters instead of phi nodes, so we use the **Braun et al. algorithm** ("Simple and Efficient Construction of SSA Form"): while walking blocks in order, keep for each
promotable slot the "current value" per block; when a block has several predecessors and the value differs, create a block parameter and add the argument to every incoming jump;
remove trivial parameters (all arguments equal) afterwards. This needs no dominance frontier computation.

A slot is **promotable** when every use of its address value is `Load(ty, slot)` or `Store(slot, v)` with the same `ty`, and the address never appears as a call argument, a `PtrAdd`
operand, a `MemCopy` operand, a stored value, a `CallPtr` argument, or a return value. The check is a single scan over the function's operands.

Uninitialised reads: a `Load` before any `Store` on some path reads "undefined". Promote to a zero constant (the lowering always zeroes flag slots in the entry block, and
variables are always written before being read), and have the verifier option `--strict-uninit` catch any real uses in tests.

## 2.2 Split slots

Preconditions: size known, every access is `PtrAdd(slot, const)` then `Load`/`Store`, accesses do not overlap partially (same offset implies same type), and the base address is not passed anywhere (including
`MemCopy` with a non-constant length). A `MemCopy` of a whole slot to another slot of the same layout is rewritten into per-field loads and stores. After splitting, run pass 2.
This is what makes `string` (`ptr, len, cap`) and `option<T>` values live in registers.

## 2.3 The drop flags

After 2, 3 and 4, a flag is an SSA boolean. In the common case it is set to `true` right after the value is made and tested once at the end of the scope:

```
v_flag = const true         // after creation
...                         // the value is never moved on this path
branch v_flag, drop, skip   // SCCP knows v_flag is true: the branch becomes a jump
```

SCCP folds it, CFG simplification removes the dead block, and the flag disappears. The flags that survive are exactly those where ownership is dynamic (a value moved on only some
paths). Those are real and stay. No special "flag pass" is needed; the generic passes do it. That is the reason for the ledger (A-03) coming first: it never creates most of the flags in the first place.

## 2.4 Inlining

- Cost model: instruction count of the callee (after its own optimisation). Inline if cost <= 20, or exactly one call site, or `@inline` (the attribute is parsed; the lowering must carry it into `Func`).
- Never inline: recursive functions (direct or through a cycle in the call graph), functions that contain `Stop` with state we want in traces (not an issue), and `@noinline`.
- Order: process the call graph bottom-up (callees first), so a callee is already optimised when it is inlined.
- Renaming: copy blocks with fresh ids; replace `Return` by a jump to the continuation block carrying the value as a block parameter; map parameters to the call's arguments; slots become the caller's slots (so promotion can run again).

# 3. Where the code goes

```
compiler/src/ir/opt/mod.rs         // pipeline, Options { level }, runs passes and verifies after each in debug/test builds
compiler/src/ir/opt/cfg.rs         // simplify CFG, predecessor lists, dominators (only if a pass needs them)
compiler/src/ir/opt/promote.rs     // mem2reg (Braun)
compiler/src/ir/opt/split.rs       // SROA
compiler/src/ir/opt/sccp.rs        // constant propagation and branch folding
compiler/src/ir/opt/dce.rs         // dead values, dead stores, copy propagation
compiler/src/ir/opt/inline.rs      // inliner
compiler/src/ir/opt/stats.rs       // counters: instructions, slots, loads, stores, calls, flags
```

`pelago ir --opt <path>` prints the optimised IR, `pelago run --opt <path>` runs it, `pelago ir --stats <path>` prints the counters before and after. Default stays unoptimised until
the differential tests have run clean for a while; then `--opt` becomes the default and `--no-opt` the escape hatch.

# 4. Stages (each is a shippable commit set)

1. **Plumbing:** `opt/mod.rs`, stats, differential test harness (run every lowering test program with and without the pipeline, compare output, result, leaks). Pass list empty. Tests green.
2. **Simplify CFG + DCE** (pass 1 and 5). Smallest and safest; proves the harness.
3. **Promote slots** (pass 2). Largest single win. Add targeted unit tests on hand-written IR: loop counter, if/else merge, nested loops, a slot read before written, a slot whose address escapes (must not be promoted), a slot used with two types (must not be promoted).
4. **SCCP** (pass 4). Unit tests: constant branch, flag folding, unreachable block removal, loop that cannot be folded.
5. **Split slots** (pass 3). Unit tests: struct of two ints, tuple, `string` header, `option<int>`, overlapping access (must not split).
6. **Inline** (pass 6). Unit tests: leaf function, function with two returns, recursive (not inlined), call inside a loop.
7. **Metrics and report:** a table in the card and in `ir.md` with counters for each sample program before and after.
8. **Default on.**

# 5. Targets (how we know it worked)

Measured on the sample programs and the benchmark set below, **per function, after optimisation**:

- `fn fib(n: int) -> int` has **zero** slots, zero flag operations and no calls except the two recursive ones.
- A counting loop (`var t = 0; for i of 0..n { t += i }`) has no `Load` or `Store` at all inside the loop.
- A program with only strings and lists, and no conditional moves, has **no drop flag stores**.
- `xs.len()` on a list inlines to one `Load`.
- Total instruction count of the sample programs drops by at least half.
- The differential tests show zero behaviour change.

Add three **micro-benchmarks** to `compiler/benches/` (plain `fn main` programs timed by `pelago run --time`): fib(30), a nested loop sum of 10^7, and building and summing a list of 10^6 ints. The interpreter's
step counter (instructions executed) is the metric, because wall time in an interpreter is noisy.

# 6. Things that must not be broken

- The interpreter's leak, double-free and use-after-free checks run on optimised code too. A pass that removes a drop breaks them, which is the intended safety net.
- Panics, `Stop` messages and checked arithmetic keep their order relative to side effects. Never hoist a checked operation above a store or a call, and never delete one whose result is unused unless it cannot stop (a plain `Add` can be deleted; `AddChecked` cannot).
- Debuggability: keep the function and block names; `pelago ir --opt` output must stay readable.

# 7. Extra items for the list (not in the first cut)

- Null-pointer layout for `option<box<T>>` and `option<obj>` (no tag byte), after A-02.
- Loop-invariant code motion and strength reduction.
- Common subexpression elimination for pure operations.
- Escape analysis to turn heap `box(v)` and closure environments that do not escape into stack slots.
- Tail-call elimination as an IR pass (today only self `become` with plain numbers is a loop).

# Done means

- Stages 1 to 8 are finished; `pelago run --opt` is the default; fmt, clippy and tests green.
- The targets in section 5 are met and the table is in `ir.md` (decision IR22: "the pipeline and its order").
- Differential tests cover every lowering test and example program.
- A short "how to add a pass" note is at the top of `compiler/src/ir/opt/mod.rs`.
