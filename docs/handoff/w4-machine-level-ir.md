# W4 machine-level IR (MIR): handoff

## Goal
Replace the old per-value-slot native generators with a machine IR (MIR) plus one shared register allocator, for x86-64 and AArch64 (Linux, Windows, macOS). Perf target: every benchmark at or below both gcc -O2 and Rust -O, without dropping exact overflow traps. Design: `docs/design/mir.md` (read its Status line first), repo `pelagolang/pelago`.

## Merged
- #116, #117, #118: M0-M2. #152: M3 (floats, vectors, CheckedIf, fills; allocator with coalescing, density order, memory operands).
- #155: M6a (operand swap for commutative ops in `native/ralloc.rs`, compare-flag reuse in `x86_emit.rs`).
- #156: M5a. AArch64 integer functions via MIR: `native/a64_mir.rs`, `a64_select.rs`, `a64_emit.rs`, hook at the top of `Gen::function` in `gen_a64.rs`. Functions with floats or vectors fall back to the old generator (selector returns `None`).
- MIR is OFF by default everywhere. Switch: `PELAGO_MIR=1` (tests: thread-local `gen::MIR`). Debug: `PELAGO_MIR_DUMP=1`, `PELAGO_REG_LIMIT=n`.

## Open PRs / unpushed work
None. Everything is merged to `main`; no local work is unpushed.

## Next steps (in order)
1. **M5b, AArch64 floats and vectors.** Add float/vector selection to `a64_select.rs` (`supported()` currently rejects them), opcodes in `a64_mir.rs`, emission in `a64_emit.rs` (`Asm` in `a64.rs` already has fadd/fsub/fmul/fdiv/fmadd/fcmp/fcvt/scvtf/fcvtzs/fsqrt/frintm/frintp and vector ops). `A64::load`/`store` for Float/Vec spills are `unreachable!` and must be written. Float args use v0-v7; callee-saved are d8-d15 (low 64 bits only, so vectors are caller-saved). Mirror `x86_select.rs`/`x86_emit.rs`. Then remove `gen_a64.rs`/`regs_a64.rs` after parity.
2. **M6b, x86 rows still over +5% instructions vs the old generator** (callgrind): kostya_matmul +34% (spills loop invariants in the inner loop), base64 +12, collatz +11, interp +11, strbuild +10, levenshtein +9, fannkuch +9, blur +6, floatloop +6, mapwork +5, bf_bench +5. Tried and dropped: loop splitting (regressed levenshtein), keeping spilled invariants in a register around loops (matmul -7 points, levenshtein +12).
3. **Small-structs request:** fold loads into ALU ops and pointer stepping in selection (structlist inner loop is 17 instructions vs gcc's 13).
4. **M4:** once no row is worse than +5% (geomean is already 1.001), make MIR the default and delete `gen.rs`/`regs.rs` (and the a64 pair after M5b).
5. Update `docs/design/mir.md` status and the A-10/A-11 plan cards.

## Build and test (PowerShell friendly)
```
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
$env:PELAGO_MIR = "1"; cargo test        # whole suite through the MIR (AArch64 column needs qemu-aarch64 on Linux; set PELAGO_SKIP_A64=1 to skip it)
cargo run -- build --arch=aarch64 -o out examples/programs/tree.pel   # Linux/Windows/macOS ARM targets
```
CI (`.github/workflows/ci.yml`) has jobs check, windows, arm, windows-arm, macos; all must be green. Instruction counts: `valgrind --tool=callgrind` on built binaries (cachegrind over-counts code with mid-block overflow exits). Wall-clock benches are noisy.

## Rules of the repo
Branches `claude/<type>/<short-name>` style (`<author>/<type>/<name>`); Conventional Commits for titles and commits; fill and fully tick `.github/pull_request_template.md`; no external Rust dependencies; avoid cloning big upstream benchmark repos.

## Known bugs / open questions for Dennis
- No known failing tests. Windows/macOS ARM go through the same emitter but are only exercised by CI.
- Loop heads are not aligned on AArch64 (x86 aligns to 16).
- `a64_select.rs` drops the madd/msub opcodes' use (defined and emitted, never selected).
- Decide whether to keep the old generator as a fallback for unsupported functions after M4, or require full MIR coverage.
- Pending earlier items: Goose-comparison adoption order (sqrt intrinsics, bounds-check elimination, arena allocator, ...), and the `float.sqrt` software loop in `compiler/core/core.pel` (nbody far behind gcc); see `docs/plans/bench-suite-findings.md`.
