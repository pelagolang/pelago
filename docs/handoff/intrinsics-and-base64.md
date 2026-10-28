# Handoff: float intrinsics and base64 runtime (thread "Add sqrt and rounding intrinsics")

## Goal
Make Pelago's native code as fast as gcc -O2 / rustc -O without dropping exact overflow traps. This thread: (1) hardware sqrt/floor/ceil/round, (2) investigate why `base64` is slow (runtime, not compile time).

## Done and merged
- pelagolang/pelago PR #142 `perf(core): add sqrt, floor and ceil intrinsics`, merged (squash 46cbd5b). New IR op `FUn` (Sqrt, Floor, Ceil) in `compiler/src/ir/mod.rs`; x86-64 `sqrtsd`/`roundsd` (`native/gen.rs`, `x86.rs`), AArch64 `fsqrt`/`frintm`/`frintp` (`gen_a64.rs`, `a64.rs`); `rt_sqrt/rt_floor/rt_ceil` in `compiler/core/rt.pel` + `lower/intr.rs`; `float.sqrt/floor/ceil/round` in `compiler/core/numbers.pel`. The MIR selector (`x86_select.rs`) declines functions with `FUn`, like `Fma`.
- Behaviour change: floor/ceil of NaN is NaN and exact for huge floats (test `float_to_int_saturates` updated). nbody (small) native: 0.035 s vs gcc -O2 0.015 s (was ~73x behind).

## Open PRs / unpushed work
None. A local branch `claude/perf/base64-runtime` was created from main with no commits.

## base64 findings (no code changed)
- x86 Linux container: Pelago native 0.333 s (user 0.167, sys 0.161) vs gcc -O2 0.055 s. `bench/programs/base64/pelago.pel` uses `[int]` (8-byte cells), C uses bytes: 8x memory, big page-fault/kernel cost.
- Experiment `base64-uint8-experiment.pel` (this folder, `[uint8]` lists, uses `rt_to_byte` because the language cannot narrow int to byte): 0.183 s, sys 0.030 s. User time still ~3x gcc (0.150 vs 0.052).
- Optimised IR (`pelago ir --opt`) shows `push` inlined, header in registers. Per pushed byte remains: capacity check, `add_checked`, a redundant `mul_checked` (len*elemsize, cannot overflow when len < cap), bounds check per index.

## Next steps (in order)
1. Ask Dennis to decide the spelling for narrowing int to a byte/int8..int32 (`to` rejects it; `docs/design/basics.md` section 6 leaves it open). Recommendation: `n.wrapToByte()` (wrapping) and `n.toByteChecked()` returning `option`. Record in `docs/design/basics.md` and the `ROADMAP.md` decisions table in the same commit.
2. Rewrite the base64 (and other) twins to byte lists once that exists; check `bench/harness/run.py base64` output still matches `expected*.txt`.
3. Drop the `mul_checked` on list push via a range fact (`ir/opt/ranges.rs`), and merge the several capacity checks of consecutive pushes into one reserve.
4. Bounds-check elimination, then opt-in `checkfree` (Goose comparison items; see project memory).

## Build and test (PowerShell)
```
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run --release -- build -o nb.exe bench\programs\nbody\pelago.pel
cargo run --release -- ir --opt bench\programs\base64\pelago.pel
python bench\harness\run.py --runs 7 base64
```
Targeted: `cargo test --lib native_agrees_on_sqrt_and_rounding`.

## Rules to keep
Branches `claude/<type>/<name>` (or `<author>/<type>/<name>`); Conventional Commits for titles and commits; PR template fully ticked (`.github/pull_request_template.md`); AArch64 backend (`gen_a64.rs`, `regs_a64.rs`) stays small bug-fix only until machine IR M5; no external crates; do not clone the big upstream benchmark repos.

## Known issues
- AArch64 FUn path only verified by golden encodings and CI qemu column, not timed on real ARM.
- Printing a huge float (e.g. 1e30) saturates to 9223372036854775807 (pre-existing display issue).
