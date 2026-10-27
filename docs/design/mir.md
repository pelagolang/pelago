# Machine IR (work item W4)

Cards: [A-11](../plans/cards/A-11-native-speed.md) (speed), [A-10](../plans/cards/A-10-aarch64.md) (AArch64, option 2). Work item W4 in [a11-work-items.md](../plans/a11-work-items.md). Today's backend is described in [backend.md](backend.md). This page is the plan for the layer between the IR and the bytes; it is a draft and ends with questions.

## 1. Why

Today there are two generators, `native/gen.rs` (2,200 lines, x86-64) and `native/gen_a64.rs` (1,800 lines), and two allocators, `native/regs.rs` (780 lines) and `native/regs_a64.rs` (550 lines). Each goes from IR values straight to bytes with templates. Three costs follow:

1. **The AArch64 backend lags.** `regs_a64.rs` gives a value one register for its whole life and never splits it; `regs.rs` has live ranges with holes, splitting around calls, hints and a cost model. On the ARM runners (`perf/arm-timings.md`) the geometric mean is 1.5x C `-O2` against 0.94x on x86-64. Every x86 improvement has to be written a second time.
2. **Nothing can look at the machine code before it is bytes.** Facts such as "this value was just compared", "this register already holds that constant" or "this jump goes to a block that tests the same flag" are rediscovered inside templates (`flags`, `fused_cmp`, `Fold`, `stored`, `guard_jump`), or not at all. The remaining gaps need exactly that kind of pass.
3. **Each template pins registers by hand.** `rax rcx rdx` are scratch in every template (rule 3 in `a11-work-items.md`), so every operation pays for the worst case of the instruction that needs them (`div`, shifts, `mul`).

The target is the same as A-11: every benchmark at or below **both** gcc `-O2` and Rust `-O`, with exact overflow traps.

What W4 does **not** fix by itself, so nobody expects it to: `resultpath`, `structlist` and `strsplit` are mostly IR work (small `result` and struct values in registers, threading a branch on a tag, `iter<str>` without a list); that is the "Small structs in registers" thread. W4 gives that work a better place to land (see section 7) and makes the ARM rows follow the x86-64 ones.

## 2. Shape

```
IR (verified, optimised)
  |  select  (one per CPU: x86-64, AArch64)      native/x86/select.rs, native/a64/select.rs
  v
MIR  virtual registers, machine opcodes, blocks   native/mir.rs
  |  pre-allocation passes (shared)              native/mir_opt.rs
  |  allocate (shared)                           native/ralloc.rs
  |  post-allocation passes (shared)             native/mir_opt.rs
  v
  emit  (one per CPU)                            native/x86/emit.rs, native/a64/emit.rs
  v
bytes (the existing x86.rs / a64.rs encoders, elf.rs, pe.rs, macho.rs, rt*.rs stay as they are)
```

### MIR

A function is a list of blocks in layout order; a block is a list of instructions ending in a branch. Not SSA: a virtual register may be written more than once (a block parameter becomes a copy at the end of each predecessor, as `edge_copies` does today, but explicit and visible to the allocator).

```rust
pub struct MFunc { pub name: String, pub blocks: Vec<MBlock>, pub vregs: Vec<VClass>, pub frame: Vec<FrameObj>, ... }
pub struct MInst { pub op: u16 /* target opcode */, pub ops: Vec<Operand>, pub flags: IFlags /* call, branch, may-trap, clobbers-flags */ }
pub enum Operand {
    Reg(Reg, Role),                  // Reg = Virt(u32) | Phys(u8); Role = Def | Use | UseDef
    Imm(i64), Frame(FrameId, i32),   // an address in the frame
    Mem { base: Reg, index: Option<(Reg, u8)>, disp: i32 },
    Block(u32), Sym(Symbol),
}
```

- **The opcodes belong to the target**; the operand shapes and roles are shared. Each target implements a small trait (`operand roles of an opcode`, `is a copy`, `is a branch`, `clobbers`) so the shared passes need no knowledge of x86 or ARM.
- **Two-address and fixed registers are constraints, not templates.** An operand can be tied (`dst = dst op src` on x86) or fixed (`idiv` reads `rdx:rax`, shifts read `cl`, calls use the argument registers and clobber the caller-saved set). The allocator honours them by inserting copies it then tries to coalesce.
- **Flags are modelled, not implicit.** `cmp`, `jcc`, `setcc`, `cmov` and the `add`+`jo` of a checked operation are selected as macro-instructions (`CmpBr`, `CmpSel`, `AddChk`) that read vregs and have no flags dependence between instructions. They expand at emission. That keeps the allocator free to move code between them and keeps `jo`-to-stop exact.
- **Classes:** `Int` (integers, bools, pointers), `Float` (f32, f64) and `Vec` (v2i64). Each target says which registers are in each class, which are caller- and callee-saved, the argument and result registers, and the registers the encoders need as scratch (one, reserved, so no template needs `rax` any more).

### Allocation (`ralloc.rs`)

One linear-scan allocator with the features `regs.rs` has today (live ranges with holes, weights by loop depth, hints from copies and from fixed uses, caller-saved registers when no call is crossed, splitting around calls instead of giving up, callee-saved registers priced by their save and restore), generalised over the register file of a target. It also does what the generators do around it today: spill slots, reloads, parallel moves for block edges and call arguments (one implementation, cycle-breaking with the reserved scratch register instead of the stack), prologue and epilogue shapes (`frameless`, `light`, framed), and shrink-wrapping later.

Spilled values are reloaded into the scratch register at the use (as today) first; a second allocation round that gives reload registers to the allocator comes after parity.

### Passes on MIR

Before allocation: copy propagation, constant materialisation sunk to its use or hoisted out of loops by cost, address-mode folding for what the selector did not fold, redundant `cmp` removal. After allocation: copy and move cleanup, branch threading and block layout with real fall-through, alignment of loop heads and `jcc` guard (the 32-byte rule from `guard_jump`), jump-to-jump removal, and a tail of target peepholes (`test r,r` for `cmp r,0`, `lea` for `add` into a new register, `xor r,r` for a zero). They run per function and are cheap (one pass over instructions), because compile speed is a project goal.

## 3. What stays

The IR and its optimiser, the encoders (`x86.rs`, `a64.rs` and their golden tests), the object writers, the runtimes, `Uses`, the guard-page and fault-handler code, the calling conventions in `backend.md`, the differential tests (`lower/tests.rs`, `native/tests.rs`, `ir/opt/tests.rs`), `REG_LIMIT` (it moves to `ralloc.rs`), and `compare.sh`. Rules 1 to 6 of `a11-work-items.md` stop being rules of a template generator and become properties the allocator and the selector are tested for.

## 4. How we get there without a flag day

**Status:** M0 merged (PR #116). M1 built: `native/mir.rs` (types, liveness, verifier, printer) and `native/ralloc.rs` (the allocator), tested in `native/ralloc_tests.rs` on a toy target with an interpreter that runs random programs before and after allocation with 0, 1, 2, 3, 4, 6 and all registers per class. M2 merged (#118). M3a built on `claude/perf/mir-floats`: floats, vectors, `CheckedIf` and fill loops through the MIR, with copy coalescing, density ordering and memory operands in the allocator; the whole suite passes with `PELAGO_MIR=1`. MIR stays opt-in: on instruction counts (valgrind, 41 bench programs) it is +3.6% geomean against the old generator, with the worst rows collatz, resultpath, nbody, matmul, crc32 and bf_mandel (+19 to +33%), and wins on lru, nqueen, graph, tak and strsplit. Loop-invariant register splitting was tried and dropped: it fixed matmul and broke levenshtein. The gap is the loop-invariant and addressing work of M6 (hoisting, load folding, pointer stepping, as asked for `structlist`), so M3 is closed at opt-in and M4 waits for the gate.

The new path is built **beside** the old one and takes over function by function.

| Step | Branch | What | Merge gate |
|---|---|---|---|
| M0 | `claude/docs/mir-design` | This page, the work-item text, the cards. | Docs only. |
| M1 | `claude/refactor/mir-core` | `mir.rs` (types, builder, printer, verifier), `ralloc.rs` (liveness, linear scan, parallel moves, spill code) tested on a toy target with a small register file; `pelago mir` is not added yet. No generator uses it. | `cargo test`; no change in generated code. |
| M2 | `claude/perf/mir-x86-select` | The x86-64 selector and emitter for integer code (arithmetic, compares, branches, calls, loads, stores, slots). A function that uses an instruction the selector does not know yet goes through the old generator (`PELAGO_MIR=0|1|auto`, default off). Both emit into one `Asm`, the calling convention is the same. | Differential tests with MIR forced on pass for every program the selector accepts; default stays off. |
| M3 | `claude/perf/mir-x86-full` | The rest on x86-64: floats and `xmm` in registers, vectors, `CheckedIf`, fill loops, memcopy, print, alloc, `Fma`, casts. MIR on by default when it is no slower. | Whole suite on both modes; `compare.sh --checked` geometric mean not worse than the old path by more than 1 percent and no row worse than 5 percent (the box is noisy; best of 15). |
| M4 | `claude/perf/mir-x86-only` | Remove `gen.rs` and `regs.rs`. | Same. |
| M5 | `claude/perf/mir-aarch64` | The AArch64 selector and emitter on the shared allocator (Linux, Windows, macOS). Remove `gen_a64.rs` and `regs_a64.rs`. | Differential tests under qemu and on the ARM runners; `bench-arm` numbers in the PR. |
| M6 | `claude/perf/mir-passes` | The passes in section 2, measured one by one against the rows that motivated them. | Each pass lands with its benchmark delta. |

M2 to M4 can be one PR each or fewer if the diff stays readable. M5 touches `gen_a64.rs`, so it waits for the AArch64 thread to be at rest, or rebases onto what it pushes.

## 5. Risks and how they are held

- **Regressions on hand-tuned benchmarks.** The old path stays until the new one matches it row by row; the merge gate is a table in the PR. The benchmark noise is 5 to 20 percent on 20 ms programs, so gates use best of 15 and the geometric mean.
- **Subtle template knowledge is lost.** Examples: a result sharing an operand's register, `CheckedIf` stubs that keep every register as it was, the large-slot `rep stosb` saving `rdi rsi`, the stack alignment around calls with pushed values, loop alignment that moved `collatz` by 40 percent. Each one gets a test in `native/tests.rs` before the code that has to preserve it, and `REG_LIMIT` runs every program with 1, 2 and 4 registers per class.
- **Compile time.** MIR is a second pass over the function. Budget: the whole native path stays within 25 percent of today's time on the largest example (`lex-bench` and `build` timings in the PR).
- **Overlap with other threads.** M1 and M2 add files and touch `mod.rs` and `main.rs` by a line each. Changes other threads make to `gen.rs` in the meantime keep working (the fallback is the old generator) but must be ported at M3; the thread doing that says so in its PR.

## 6. Tests

- MIR verifier (every block ends in a branch, every vreg has a class and is defined before use on every path, operand roles match the opcode) run after selection, after each pass and after allocation in test builds.
- Allocator tests on hand-written MIR: interference, splitting around a call, fixed registers (`idiv`), tied operands, parallel-move cycles, 1-register and 2-register limits.
- The existing differential tests (interpreter against native, plain and optimised, Linux x86-64, AArch64 under qemu, Windows in CI) with `PELAGO_MIR=1`; CI runs them in both modes until M4.
- Golden MIR dumps for a few small functions (`fib`, a loop, a checked add, a division) so a change in selection shows in review.

## 7. Where the speed comes from (and where it does not)

| Gap | Cause today | Does W4 help? |
|---|---|---|
| `fib38` (1.37x x86-64, 2.5x ARM Linux) | callee-saved pushes in the base case, a constant (`i64::MAX`) re-materialised per inlined level, `cmp`/`jcc` through scratch | Yes: post-allocation cleanup, shrink-wrapping, constants hoisted by cost (M6); on ARM the shared allocator (M5) is most of the gap |
| ARM rows (collatz 2.5x, divmod 2.1x, fib 2.5x) | `regs_a64.rs` has no splitting, hints or caller-saved cost model | Yes, mostly M5 |
| `resultpath` (3.7x) | tag computed and compared at every `?`; the branch on it is not threaded | Partly: threading a branch on a known tag is a MIR pass (M6) only if the IR thread has not removed it first |
| `structlist` (2.4x) | per-element address recomputed, two checked adds, no vectorisation | Partly: address folding and redundant-check cleanup (M6); the rest is IR |
| `strsplit` (2.0x) | library: an `iter<str>` list and a `result` per number | No: library and IR |
| `switchwork` (1.2x) | jump table and enum payload loads | Partly: the selector can emit a table jump and load folding |

## Questions

1. **Scope.** Replace both generators as in section 4 (recommended), or stop after M4 (x86-64 only) and decide on AArch64 afterwards?
2. **Gate.** Is "geometric mean within 1 percent and no row more than 5 percent worse, best of 15" the right bar for turning MIR on by default, given the target of every row at or below C and Rust?
3. **Ordering with the AArch64 thread.** M5 rewrites `gen_a64.rs`. Should the AArch64 thread keep adding features to `gen_a64.rs` meanwhile (they are then ported at M5), or freeze it?
4. **Flag name.** `PELAGO_MIR=0|1|auto` for the transition (environment variable, removed at M4), or a `build --mir` option?
