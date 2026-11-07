# Benchmarks still behind C or Rust: plan per row (2026-11-07)

Standing rule (Dennis, 2026-11-07): when a row is still worse than C, Rust or C++, write the plan for getting it lower. The second rule: use Pelago's own semantics, not C's methods, and say where.

Source of the numbers: `python3 bench/harness/run.py --runs 5 --size medium` on a shared 4-core container, Intel Xeon, gcc 13, rustc 1.97, main at `458ac9f`. Rows marked `!` in the harness have a spread over 10 percent, so any ratio is good to about plus or minus 15 percent; the Ir counts below (callgrind) are exact. Result: geometric mean of Pelago over the best other row 1.34x, at or below every other row on 10 of 40. Rows at or below all rivals: base64 0.88x, divmod 0.94x, lexer 0.74x, list_sum 0.57x, primes 0.97x, sieve 0.75x, sortwork 0.73x, strbuild 0.82x, structloop 0.58x, tak 0.78x (hello is start-up only).

"Diagnosed" means I read the machine code or the source; "not yet" means the cause is a guess and step 1 is the diagnosis (callgrind per function plus disassembly of the hot loop).

## Pelago-specific levers (what C cannot do)

1. **Unique ownership gives free alias facts.** A `var` list or struct is not aliased by anything (value semantics, `inout` and `sink` repeated at the call site). So `a[i]` read and written in an inner loop can live in a register (scalar promotion), a stream of `bs[i].x` and `bs[i].y` can be vectorised with no runtime overlap test, and a slice's data pointer and length can be loaded once per function. C and Rust need runtime alias versions or `restrict`/borrow proofs. The code today still emits runtime overlap tests (`cmovbe` chains seen in levenshtein) where the type system already proves them away.
2. **Exact traps are a range question, not a flag per add.** Ranges, loop caps and `CheckedIf` already remove most checks; more of the loop-counter proofs (`@checkfree` style) make the remaining ones free.
3. **Region heap.** Allocation-heavy rows (bintrees, lru, wordfreq, knucleotide) should use region bump allocation with no per-node free, which a language with no aliasing and no finalisers can do soundly.

## Rows, worst first

| row | vs best other | cause | plan |
|---|---|---|---|
| bintrees | 5.3x | not yet diagnosed; C++ expert arena is 0.013 s, plain C 0.038 s, Rust 0.087 s. Pelago 0.069 s sits between: per-node alloc and free calls. | Lever 3: a function that builds a tree and returns it allocates from a region owned by the result; freeing the root frees the region in one step. The arena thread's region heap is the base; the missing piece is a lowering rule for `obj`/`struct` allocation inside a function whose result owns it. Target below the C 0.038 s first. |
| levenshtein | 4.3x | diagnosed: `dist` is inlined into `main`, which leaves a frame over 0x390 bytes and the hot loop reloads `prev`, `cur`, lengths and base pointers from stack slots every pass. Six bounds checks per pass (`a[i]`, `b[j]`, `prev[j]`, `prev[j+1]`, `cur[j]`, `cur[j+1]`), and the fast copy also carries two runtime overlap tests that value semantics already rules out. | (a) Do not inline a function into a loop's caller when it has loops and more than N live values (keep `dist` a call); (b) drop the overlap tests for lists that are distinct locals (lever 1); (c) hoist data pointers and lengths of unique lists; (d) promote `cur[j]` to a register across the j step (it is stored and read back one pass later). Expected Ir cut in half; then the loop is bound by the 3 loads and 1 store, like clang's 0.209 s. |
| structlist | 3.6x | diagnosed from the source: `bs[i].x`, `.y` updates with `& 65535`; gcc and clang vectorise the pair. Pelago's vectoriser (`opt/vectorise.rs`) takes `xs[i]` over element lists, not fields of a struct element. | Extend the vectoriser to a stride-4 struct list: treat the four fields as a 4x i64 row, load two fields as one `v2i64`. Lever 1 removes the overlap test C pays. Loop 20000 x 1000 passes, target below gcc's 0.011 s. |
| nbody | 2.3x | not yet diagnosed (float code; C 0.015 s). Probable: field loads and stores on struct bodies that a unique-ownership analysis could keep in xmm registers, and `sqrt` plus divide latency. | Step 1: disassemble the pair loop. Likely work: scalar replacement of fields of a unique `var` struct across the loop, vectorising the three coordinate lanes. |
| resultpath | 2.3x | known from the A-11 card: gcc folds `(x >> 1) * 3 + 1` into one `lea` and computes the multiply only on the side that needs it. | `lea` selection for `x*3+1` after the shift (a MIR peephole), and sinking of the arm's arithmetic once it is a conditional `lea`. |
| lru | 2.2x | not yet diagnosed. Map operations dominate. | Step 1: callgrind by function; expect hashing and the list-of-nodes updates. Lever 3 applies to the node allocation. |
| matmul | 1.8x | not yet diagnosed (gcc 0.015 s, trapv 0.013 s: vectorised). | Extend the array-loop vectoriser to a nested 2D access pattern; the checked-sum guard already exists for 1D. |
| switchwork | 1.8x | not yet diagnosed (clang 0.029 s). Probably the jump table vs compare chain choice. | Step 1: look at the switch lowering: a dense `switch` should be one bounds test plus an indirect jump; a sparse one a binary tree. |
| loops | 1.6x | diagnosed from the source: `a[i] = a[i] + j % u` with `u` loop-invariant but not constant; gcc's 0.333 s equals Pelago's 0.330 s because both execute a 64-bit `idiv` per pass, clang 0.201 s avoids it. | Two fixes. (a) Scalar promotion of `a[i]` across the inner loop (lever 1; a unique list cannot change under the loop). (b) A 32-bit `div` when dividend and divisor both fit in 32 bits, tested by one `or` and a shift; with value ranges known (`u` from `20 + s % 30`, `j` below 10000) the test disappears and the 32-bit divide is unconditional. That is range-driven, not a copy of clang's runtime bypass. |
| wordfreq | 1.6x | not yet diagnosed. | Callgrind by function: hashing, string compare and map growth are the candidates. |
| fib38 | 1.6x | known: gcc 0.072 s is probably unrolling and turning one call into a loop (not checked); clang 0.120 s is level with Pelago. | Partial recursion-to-loop on the second call; only if it keeps the exact overflow stop. Lower priority since clang and Rust-checked are level or slower. |
| blur | 1.6x | not yet diagnosed (clang 0.022 s). | Vectorise the 3-point stencil; shares work with matmul. |
| knucleotide, fasta, collatz, kostya_matmul, kostya_bf_bench, fannkuch, interp, nqueen | 1.3 to 1.5x | collatz: Rust 0.049 s is the best; the select-vs-branch decision was already tuned. interp: Pelago 0.075 s vs gcc 0.054 s: a switch dispatch loop, so the same switch lowering item as switchwork. bf_bench: also a dispatch loop. The rest not yet diagnosed. | One diagnosis pass each, in this order: interp and bf_bench with switchwork (same code path), then fannkuch and nqueen (array loops: lever 1), then knucleotide and fasta (map and string work). |
| kostya_json, strsplit, mapwork, histogram, spectralnorm, mandelbrot, graph | 1.2 to 1.3x | within run-to-run noise on this machine for several (all marked `!`). | Re-measure on the quiet machine before spending effort; only then diagnose. |
| crc32, loop_sum, floatloop | 1.0 to 1.1x | level within noise (loop_sum and floatloop are latency-bound chains, already minimal). | None now. |

## Back-end work (W4) behind several rows

- **Lexer (now ahead of C and Rust at 0.74x of the best)**: after PR 204, remaining waste is codegen: setcc/movzx chains for letter and digit tests, flags kept as bools. The audit to allow RAX/RCX/RDX/R11 as allocatable is still open; it only matters for rows with more than 11 live values, which is levenshtein's inlined `main`, not the lexer any more.
- **base64 (now ahead at 0.88x)**: live-range splitting at the guard landed, the redundant `zext` through `rax` is a one-line peephole to do next.
- **Write sets**: widening the number of rows that benefit needs an exact write set per runtime routine and a fixed point for recursive functions (see `docs/design/mir.md`, section on write sets). The measurement in PR 199 found only 2 of 40 benchmarks change, so this stays below the rows above.

## Order of work

1. levenshtein (a, b, c, d): fixes the stack-slot pressure that also hits other inlined hot loops.
2. loops (a, b): small, range-driven, uses lever 1.
3. structlist and matmul and blur: the vectoriser extension (stride-4 struct, 2D, stencil).
4. bintrees, lru: region allocation in the result-owning function.
5. switchwork, interp, bf_bench: switch lowering.
6. Diagnose nbody, wordfreq, knucleotide, fasta, fannkuch, nqueen.
