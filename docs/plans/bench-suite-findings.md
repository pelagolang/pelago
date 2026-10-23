# Benchmark suites: coverage, findings, missing features

Written 2026-10-23 while adding the programs of the suites that goose's `bench/` draws on. Rule from Dennis: **do not optimise speed in the benchmark work**; the findings below are for other threads to pick up when he says it is time.

## Where the programs come from

Programs from `bddicken/languages` (no licence file) are written from the public description of the problem (algorithm, sizes, checksum), never copied. Programs from `kostya/benchmarks` (MIT) follow its sources with attribution. Each program has the four twins (`pelago.pel`, `c.c`, `cpp.cpp`, `rs.rs`), prints one agreed output and has `small` / `medium` / `large` sizes. Output is integers or `%.6f` floats so every language prints the same text.

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
| kostya (`kostya/benchmarks`, MIT) | brainfuck `bench.b` and `mandel.b` | `kostya_bf_bench`, `kostya_bf_mandel` (op-tree interpreter after `bf.c`, the real `.b` programs; mandel takes about 20 s per run, so it carries a `heavy` marker and runs only when named or with `--heavy`) |
| kostya | base64 | `base64` (131072 bytes of `a`, padding, fresh buffers; 256 tries, the suite uses 8192) |
| kostya | matmul | `kostya_matmul` (float, transposed second matrix; `calc(400)`, the suite uses 1500) |
| kostya | primes | `primes` (sieve of Atkin to 5,000,000, digit trie, breadth-first walk from prefix 32338, sorted) |
| kostya | json | `kostya_json` (20000 records built in memory; hand-written parser into a tree; averages x, y, z; the suite parses 524288 records from a file) |

The kostya repository has exactly these five tests (goose's `bench/kostya/README.md` talks about a 50-benchmark suite and points to a repository called LangArena; that is a different project from `kostya/benchmarks` and I did not read it). It is MIT licensed, so the Brainfuck programs and the structure of each test are used with attribution in the file headers.
| goose's own | sum, push, rng, tree, strlist, words, records, particles, scene, sexp, calc, respond, bintrees, blur, graph, interp, lru | the generic ones exist (`loop_sum`, `list_sum`, `wordfreq`, `structlist`, `bintrees`, `blur`, `graph`, `interp`, `lru`); `scene`, `sexp`, `calc`, `respond`, `particles` are goose-specific designs and not added |

Not yet blessed or run for all three sizes: the harness run was stopped on Dennis's request. `expected.txt` (medium) of every new program was checked against the C twin; `small` / `large` files exist only where the stopped run got to them, and the harness falls back to the reference row for the rest.

## Findings (for the speed thread, later)

1. **`float.sqrt` is a software loop.** `compiler/core/core.pel` computes it with 80 Newton iterations (about 780 ns per call). `nbody` is 70x to 95x slower than gcc -O2 because of it, and `spectralnorm` and any distance computation pay too. The fix is a backend instruction (`sqrtsd` on x86-64, `fsqrt` on AArch64, which `a64.rs` can already encode), wired the way `rt_fma` is. The same goes for `floor` / `ceil` / `round`, which round through `rt_to_int`.
2. `str.chars()` builds a full list before iterating (seen in `lexer`, 5x behind at the large size).
3. `levenshtein` is 4x behind gcc and clang at medium: two heap rows per call and bounds checks in the inner loop.
4. `loops` is 1.7x behind clang and rustc and level with gcc -O2 (they vectorise or strength-reduce `j % u`).
5. `base64`, `knucleotide`, `primes`, `nqueen`, `fasta`, `kostya_*`: run and agree with C. No timing table was collected for them.

## Features Pelago lacks (found while writing the twins)

Needed to port the remaining suite programs, in the order they would unlock work:

| Missing | Unlocks |
|---|---|
| `loop` and `obj` are reserved words, so an enum case or field cannot be called that (the kostya interpreter's `loop` op became `again`, the JSON `obj` case `dict`) | none: only a naming note |
| Float printing trims trailing zeros (`4.19036`), so the C, C++ and Rust twins must trim the same way | a `%.6f`-style formatter |
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
