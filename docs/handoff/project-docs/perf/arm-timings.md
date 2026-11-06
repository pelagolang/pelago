# Pelago on ARM hardware, 2026-10-21

Pelago AArch64 backend (`--opt`, registers on) against C `-O2` and Rust `-O`, on the GitHub ARM runners (`bench-arm` workflow, run 39573414668, best of 5, seconds). C is gcc on Linux and clang on macOS and Windows. Ratios are Pelago divided by C `-O2` (below 1.00 is faster than C). Every program printed the same output as its C twin.

| program | Linux pelago | vs C | vs Rust | macOS pelago | vs C | vs Rust | Windows pelago | vs C | vs Rust |
|---|---|---|---|---|---|---|---|---|---|
| collatz | 0.110 | 2.54x | 4.21x | 0.129 | 2.40x | 3.09x | 0.123 | 3.10x | 3.04x |
| divmod | 0.051 | 2.10x | 2.07x | 0.037 | 1.87x | 1.76x | 0.064 | 1.68x | 1.61x |
| fib38 | 0.178 | 2.55x | 2.00x | 0.160 | 1.11x | 1.22x | 0.190 | 1.96x | 1.83x |
| floatloop | 0.111 | 1.50x | 1.43x | 0.149 | 1.78x | 1.17x | 0.124 | 1.39x | 1.35x |
| list_sum | 0.021 | 1.24x | 1.18x | 0.036 | 3.75x | 2.17x | 0.044 | 1.29x | 1.20x |
| loop_sum | 0.359 | 1.33x | 1.35x | 0.414 | 1.35x | 1.34x | 0.372 | 1.33x | 1.32x |
| mapwork | 0.017 | 2.01x | 1.18x | 0.012 | 2.83x | 1.20x | 0.029 | 1.37x | 0.93x |
| sieve | 0.032 | 0.72x | 0.63x | 0.050 | 1.25x | 1.19x | 0.064 | 1.38x | 1.37x |
| strbuild | 0.005 | 1.75x | 0.85x | 0.005 | 1.78x | 0.77x | 0.018 | 1.04x | 0.88x |
| structloop | 0.013 | 1.27x | 3.02x | 0.018 | 1.44x | 4.02x | 0.026 | 1.41x | 1.38x |
| tak | 0.086 | 0.91x | 0.74x | 0.090 | 0.93x | 0.89x | 0.099 | 0.79x | 0.77x |
| geometric mean | | 1.52x | 1.44x | | 1.72x | 1.50x | | 1.44x | 1.33x |
| at or below C -O2 | | 2 of 11 | | | 1 of 11 | | | 1 of 11 | |

Caveats: shared runners are noisy, and most programs run for 5 to 50 ms, so single rows can move by 20% or more between runs. Treat the geometric means and the large gaps as the signal.

Large-block growth: `list_sum` pushes 5 million ints (a 40 MB list). On Linux the runtime grows it with `mremap` (1.24x of C). On macOS and Windows it copies on every growth step: 3.75x on macOS (C's `realloc` there moves pages without copying) and 1.29x on Windows (C's `realloc` copies too). The other programs hold blocks under 10 MB and are not affected.
