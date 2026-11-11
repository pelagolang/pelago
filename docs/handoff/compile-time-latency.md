# Handoff: cut Pelago compile-time latency

## Goal
`pelago build` of `bench/programs/base64` takes ~60-70 ms; tcc takes ~6 ms. Reduce compile time without dropping overflow traps or other semantics. The harness number (`bench/harness/run.py`) is the whole `pelago build` run (no run time, no subprocesses; Pelago writes the ELF itself).

## Findings (release build, x86-64 Linux, `pelago build --timings`)
- base64: ~66 ms total, of which the IR optimiser is 52-57 ms. Everything else is ~12 ms.
- hello/divmod/collatz/list_sum: 20-25 ms total. Fixed cost on every program is ~13-15 ms: load+parse core ~3, resolve ~2, type check 4-7, value checks ~2. Optimiser 4-9 ms. Native codegen + write < 1.5 ms.
- Inside the optimiser on base64, `ivopt` is ~25 ms (96 runs). `try_loop` clones the whole Func (about 1,100 insts after inlining) per rewrite candidate, runs `dce` on the clone, and `weight()` clones and runs `dce` again. Then simplify ~5 ms, ranges ~4 ms, dce ~2 ms. SipHash and malloc are also large.

## Merged
- PR #144 `perf(cli): print per-phase compile times with --timings` (also prints the 8 slowest optimiser passes). Use: `pelago build --timings -o out file.pel`.

## Open / not yet a PR
- PR #146 (branch `claude/perf/compile-time`, from another thread): FxHash for the passes (`ir/opt/fx.rs`), `ivopt` returns early when no candidate and shares preds/defs/dom between loops. Callgrind 490M -> 400M instructions. Check whether it merged. It touches many pass files; rebase onto it.
- Branch `claude/perf/ivopt-analysis-reuse` (pushed, no PR): same sharing as #146 (will conflict) PLUS one extra fix that is not in #146: `ivopt::weight()` ran `dce` a second time on a candidate already dce'd. Added `weight_live()` and use it for the candidate (`let after = weight_live(&g, lp)`). Measured base64 optimiser 58 -> 38 ms, identical binary (`cmp` same). After #146 merges, rebase and keep only the `weight_live` change, then open a PR (`perf(opt): skip a second dead-code pass when weighing ivopt candidates`).

## Next steps, in order
1. Merge/rebase as above; get the `weight_live` PR in.
2. Evaluate `ivopt` candidates without cloning the Func (compute the loop weight delta from the candidate's insts, or undo log instead of `f.clone()`). Re-check with `--timings` that `ivopt` drops.
3. Share/cached dominators and preds across passes within a round (`Dom::new`, `Func::predecessors`, `Func::defs` are rebuilt per pass and loop). Look at `dce::run` (~24% inclusive) and `simplify` (runs 192 times on base64).
4. Fixed core cost (~13 ms on every program): options are caching the parsed/resolved core, lazy-loading unused core modules, or pruning before type check/lowering. Measure with `--timings` first.
5. Optional: a fast default mode with only cheap passes; direct codegen is NOT justified since codegen is < 1.5 ms.
Targets suggested by ChatGPT (not commitments): <40 ms, <20 ms, stretch 6-10 ms for base64. Fixed 13 ms means the stretch needs step 4.

## Build / test (PowerShell)
```
cargo build --release
.\target\release\pelago.exe build --timings -o out.exe bench\programs\base64\pelago.pel
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```
Run clippy BEFORE pushing (I once pushed a clippy failure). To confirm an optimiser change is behaviour-neutral, build before/after and compare the output binaries (`fc /b a.exe b.exe`). Avoid full benchmark runs; Dennis has no usage left.

## Repo rules (from CLAUDE.md and project memory)
- Branches `<author>/<type>/<short-name>`; Conventional Commits for titles and commits; PR template (`.github/pull_request_template.md`) with every box ticked (N/A marked). Do not add dependencies. No `git clone` of upstream benchmark repos.

## Open questions for Dennis
- Which to prefer: a cached pre-checked core (bigger win, more design) vs. trimming passes? Not asked yet.
