# Benchmarks (draft 1)

Decisions (Dennis, 2026-10-23): benchmark against C **and** C++; against gcc **and** clang (C and C++), plus tcc for the compile-time axis; headline numbers from both a pinned container and a quiet machine; our own program list, grown well beyond goose's 16; target is **at or below the best of all rows**, with Pelago's exact overflow traps on.

## What the harness measures (`compiler/benches/harness.py`)

| Metric | How |
|---|---|
| Run time | median and minimum of N runs after discarded warm-ups; spread = (max-min)/median, rows above 10% are marked `!` |
| Compile time | median of M builds of the same source, source to executable |
| Binary size | file size of the executable |
| Peak memory | `ru_maxrss` from `wait4` (POSIX) |
| Correctness | every executable must print the reference output (gcc -O2, else the first available), or the row is marked `DIFF` |

Rows: `pelago`, `gcc-O0`, `gcc-O2`, `gcc-O2-trapv`, `clang-O2`, `tcc`, `g++-O2`, `clang++-O2`, `rustc-O`, `rustc-O-checked`. Missing compilers drop their rows. `-trapv` and `-checked` are the like-for-like rows with Pelago's overflow checks. tcc has no optimiser, so it is read on the compile-time table and on "time to first result", not as a run-time rival.

`compare.sh` stays as the quick check used by CI; the harness is the full suite. `Dockerfile` pins the toolchain for publishable runs (add `--pin CPU`); the `bench-harness` workflow runs it on x86 and ARM runners.

## C++ twins

`benches/cpp/` holds one C++ file per program. Ten are the C source compiled as C++ (identical code, so they only measure the C++ front end and library startup). Six use the idiomatic standard library (`vector`, `string`, `unordered_map`, `std::sort`). An expert tier (arena, flat table) comes with the larger program set.

## Sizes, expected output, results page (P4 and P5)

- `sizes.json` gives each program a `small` (fits in cache) and a `large` (does not) version as text replacements applied to every twin's source; the committed source is `medium`. `--size small,medium,large` runs several. Programs without an entry (tak, resultpath, structlist, structloop, switchwork) run at medium only for now.
- `expected/<name>.<size>.txt` holds the agreed output. `--bless` writes it from the reference row (gcc -O2, else clang, g++, rustc) on a machine you trust; later runs compare every row to the file.
- `--md FILE` writes the generated results page (environment table, then one section per size with run time, compile time, binary size and peak memory). `--site FILE` writes the medium results in the `website/content/bench.json` format, `--history FILE` appends one JSON line per run. The `bench-harness` workflow uploads all of them; copying `site-bench.json` to `pelagolang/website` `content/bench.json` is still a manual step.

## Next

1. Run the harness once, bless `expected/`, and fix what the first run shows.
2. New programs: trees, strings and hash maps, records, interpreter, graph, LRU, blur, parser; each with C, C++ (idiomatic and expert) and Rust twins and its own `sizes.json` entry.
3. Memory tiers (system malloc, arena) and an overflow-check A/B.
4. Automate the website update and a weekly history chart.
