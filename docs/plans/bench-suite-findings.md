# Benchmark suites: coverage, findings, missing features

Written 2026-10-23 while adding the programs of the suites that goose's `bench/` draws on. Rule from Dennis: **do not optimise speed in the benchmark work**; the findings below are for other threads to pick up when he says it is time.

## Where the programs come from

Every program below is written from the public description of the problem (algorithm, sizes, checksum), never copied: the `bddicken/languages` repository has no licence file, and the suites are large. Each program has the four twins (`pelago.pel`, `c.c`, `cpp.cpp`, `rs.rs`), prints one agreed output and has `small` / `medium` / `large` sizes. Output is integers or `%.6f` floats so every language prints the same text.

| Suite (as used in goose `bench/`) | Benchmark | Status in `bench/programs/` |
|---|---|---|
| bddicken | hello-world | `hello` |
| bddicken | loops | `loops` (64-bit cells, divisor derived at run time so it cannot fold) |
| bddicken | fibonacci | already `fib38` |
| bddicken | levenshtein | `levenshtein` (two-row Wagner-Fischer, words generated in the program) |
| plb | helloworld, binarytrees, fannkuch-redux, mandelbrot, lru, nsieve | already `hello`, `bintrees`, `fannkuch`, `mandelbrot`, `lru`, `sieve` |
| plb | nbody, spectral-norm, fasta, knucleotide | `nbody`, `spectralnorm`, `fasta` (checksum instead of text), `knucleotide` (hash map of k-mers) |
| plb | merkletrees | not added: same allocation pattern as `bintrees` |
| plb | json-serde, regex-redux, edigits, pidigits, secp256k1, coro-prime-sieve | **not added**, see missing features |
| plb2 | matmul | already `matmul` (integer; the suite's is float 1500x1500) |
| plb2 | nqueen | `nqueen` (bit masks) |
| plb2 | sudoku, bedcov | not added yet (longer programs) |
| kostya2 | brainfuck, base64, primes | `brainfuck`, `base64` (hand-written codec), `primes` (sieve of Atkin, digit trie, breadth-first walk) |
| kostya2 | json (typed and DOM), matmul (float) | not added yet |
| kostya (50 benchmarks) | all | **not added**: the list lives in the suite's own `run.js` / `test.js`, which cannot be read without cloning it |
| goose's own | sum, push, rng, tree, strlist, words, records, particles, scene, sexp, calc, respond, bintrees, blur, graph, interp, lru | the generic ones exist (`loop_sum`, `list_sum`, `wordfreq`, `structlist`, `bintrees`, `blur`, `graph`, `interp`, `lru`); `scene`, `sexp`, `calc`, `respond`, `particles` are goose-specific designs and not added |

Not yet blessed or run for all three sizes: the harness run was stopped on Dennis's request. `expected.txt` (medium) of every new program was checked against the C twin; `small` / `large` files exist only where the stopped run got to them, and the harness falls back to the reference row for the rest.

## Findings (for the speed thread, later)

1. **`float.sqrt` is a software loop.** `compiler/core/core.pel` computes it with 80 Newton iterations (about 780 ns per call). `nbody` is 70x to 95x slower than gcc -O2 because of it, and `spectralnorm` and any distance computation pay too. The fix is a backend instruction (`sqrtsd` on x86-64, `fsqrt` on AArch64, which `a64.rs` can already encode), wired the way `rt_fma` is. The same goes for `floor` / `ceil` / `round`, which round through `rt_to_int`.
2. `str.chars()` builds a full list before iterating (seen in `lexer`, 5x behind at the large size).
3. `levenshtein` is 4x behind gcc and clang at medium: two heap rows per call and bounds checks in the inner loop.
4. `loops` is 1.7x behind clang and rustc and level with gcc -O2 (they vectorise or strength-reduce `j % u`).
5. `brainfuck`, `base64`, `knucleotide`, `primes`, `nqueen`, `fasta`: run and agree with C. No timing table was collected for them.

## Features Pelago lacks (found while writing the twins)

Needed to port the remaining suite programs, in the order they would unlock work:

| Missing | Unlocks |
|---|---|
| Big integers in the library (`bigint`) | edigits, pidigits, secp256k1 |
| Regular expressions in the library | regex-redux |
| Coroutines or channels (concurrency design is on paper only) | coro-prime-sieve |
| JSON parse and write in the library, with typed decoding | json-serde, kostya2 json |
| Reading a file or standard input, and program arguments | suites that take an input file or size on the command line (we generate input inside the program instead) |
| `float -> int` conversion (`to` refuses it: "can lose information") with an explicit rounding form such as `floor_to_int` | printing fixed-point values, any benchmark that bins floats |
| Float formatting control (`%.9f`) | nbody and spectral-norm print six digits only |
| Fixed-width integer and byte types and byte arrays (`int` is 64-bit; every twin uses 64-bit cells, so memory is higher than the C originals use) | memory comparisons in loops, base64, brainfuck, crc32 |
| A way to stop constant folding (`black_box`) | loops and similar kernels cannot be made opaque in Pelago, so a clever optimiser may fold them |
| A statement to build a `[T]` of a given size without a push loop (`[0; n]`) | every program starts with a fill loop |

I did not check whether threads, SIMD types or `u8` exist in source form; list them if they do.

## Reminder for the coordinator

When Dennis says it is time for the speed work: open threads for (1) sqrt / floor / ceil intrinsics in the backend, (2) `str.chars()` without the intermediate list, (3) bounds-check elimination and strength reduction for the `levenshtein` and `loops` loops, then rerun `python3 bench/harness/run.py --size small,medium,large --save` for the whole suite and commit the first `bench/results/` file. Separately, ask Dennis which of the missing features above he wants scheduled.
