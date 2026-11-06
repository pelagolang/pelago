# Handoff: optimiser and native speed thread (card A-11)

Repo: `pelagolang/pelago`. Card: `docs/plans/cards/A-11-native-speed.md` (one paragraph per piece of work, kept up to date).
Standing goal from Dennis: every benchmark at or below BOTH gcc -O2 and Rust -O, with exact signed-overflow traps kept and no benchmark-specific tricks.

## Merged (all CI green)
- #61 stack huge pages
- #63 literal copies become constant stores (`opt/constdata.rs`)
- #65 add in place (`opt/undo.rs`) and trip-count caps for sums in counted loops (`opt/ranges.rs`)
- #78 loop test on a moving counter, steps and down counters (`opt/ivtest.rs`)
- #80 mapwork: wrapping index math in `compiler/core/core.pel`, `opt/devirt.rs`, first-probe split of `rt_map_find_tag`
- #83 trip counts from known ranges of limit and start; cap round repeats up to three times
- #85 `opt/fneg.rs` ((-x)*c becomes x*(-c)) and float constants hoisted by `opt/licm.rs`
- #87 blocks of 2 MiB or more mapped in whole 2 MiB pages (`native/rt.rs`); functions start 48 bytes into a 64-byte line (`native/gen.rs`)

## Open PRs and branches
None open from this thread. Local branch `claude/perf/more-benches` had no changes. Nothing is uncommitted or unpushed.

## State (compare.sh --runs 25, plain, main after #87)
vs C -O2 / vs Rust -O: collatz 0.73/1.05, divmod 0.94/0.88, fib38 1.38/0.77, floatloop 0.92/1.02, list_sum 0.77/0.77, loop_sum 1.00/1.02, mapwork 0.83/0.49, sieve 0.68/0.63, strbuild 0.99/0.44, structloop 0.34/0.56, tak 0.59/0.50. Geometric mean 0.79x C, 0.71x Rust; at or below C on 9 of 11. `--checked` was measured only for mapwork (0.92x C -ftrapv, 0.55x Rust).

## Remaining next steps
1. fib38 is the only real gap (1.1x to 1.4x). gcc unrolls about seven levels with wrapping arithmetic; checked C and Rust are slower than Pelago. Two extra inline levels were slower (register pressure). Closing it needs a wrapping fast path, which Dennis's rule forbids; probably leave it.
2. loop_sum / floatloop / collatz are within noise of C and Rust. The chains are latency-bound and equal to gcc's.
3. Run the full `--checked` table and publish it.
4. Add benchmarks with C and Rust twins: strings, sorting, `switch`-heavy code, structs in lists, `result` and `?`. The suite has only 11 loop and recursion programs.
5. Code size: function padding adds about 56 bytes per function and a small executable can grow a page (fib38 5,040 to 9,136 bytes). Look at shrinking it, and compare sizes against C.
6. Not built: zeroing the map index table in bulk, devirtualising `eq`, avoiding the header copy and write-back around calls (all in the A-11 card).

## Things to know
- Placement matters: the same fib code ran 0.099 s to 0.124 s depending on where it fell in 64-byte lines. Re-measure with `compiler/benches/compare.sh --runs 25` after any change to code size or the runtime; re-check fib38 and tak.
- Timing: use CPU time over many interleaved runs (min and median). `compare.sh` has 1 ms resolution, so short programs (mapwork, strbuild, structloop) are noisy. valgrind callgrind gives deterministic instruction counts.
- Rules: no external dependencies; commits and PR titles in Conventional Commits (`type(scope): summary`); fill every box of `.github/pull_request_template.md`; run `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` before pushing; branches `claude/<type>/<short-name>`.
