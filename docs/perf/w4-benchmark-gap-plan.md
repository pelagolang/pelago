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

- **Lexer (now ahead of C and Rust at 0.74x of the best)**: after PR 204, remaining waste is codegen: setcc/movzx chains for letter and digit tests, flags kept as bools. Allocating RAX/RCX/RDX/R11 is dropped from the plan: it only helps machines with many registers (see the register-poor section below).
- **base64 (now ahead at 0.88x)**: live-range splitting at the guard landed, the redundant `zext` through `rax` is a one-line peephole to do next.
- **Write sets**: widening the number of rows that benefit needs an exact write set per runtime routine and a fixed point for recursive functions (see `docs/design/mir.md`, section on write sets). The measurement in PR 199 found only 2 of 40 benchmarks change, so this stays below the rows above.

## Order of work

1. levenshtein (a, b, c, d): fixes the stack-slot pressure that also hits other inlined hot loops.
2. loops (a, b): small, range-driven, uses lever 1.
3. structlist and matmul and blur: the vectoriser extension (stride-4 struct, 2D, stencil).
4. bintrees, lru: region allocation in the result-owning function.
5. switchwork, interp, bf_bench: switch lowering.
6. Diagnose nbody, wordfreq, knucleotide, fasta, fannkuch, nqueen.

## Register-poor profile (Dennis, 2026-11-07: as fast as C on chips with few registers)

Gains must not depend on having more registers. `PELAGO_REG_LIMIT=n` keeps `n` registers of each class (integer and float) in the allocator; the table is instruction counts (callgrind, medium size, main at `458ac9f`) at all registers, and the ratio at 8 and at 6. Every output is identical to the full-register build. Geometric mean over 41 programs: 8 registers 1.022, 6 registers 1.061. Instructions only: spills are loads and stores that often cost little in cycles on a big core, so this is an upper bound there and a fair guide for a small in-order chip.

| program | all regs (M) | 8 regs | 6 regs |
|---|---:|---:|---:|
| base64 | 798.3 | 1.042 | 1.281 |
| bintrees | 396.9 | 0.987 | 0.986 |
| blur | 298.0 | 1.067 | 1.098 |
| collatz | 647.8 | 1.000 | 1.000 |
| crc32 | 300.0 | 1.000 | 0.867 |
| divmod | 375.6 | 1.000 | 1.000 |
| fannkuch | 293.8 | 0.994 | 1.099 |
| fasta | 338.2 | 1.150 | 1.177 |
| fib38 | 1545.6 | 0.999 | 1.100 |
| floatloop | 360.0 | 1.000 | 0.944 |
| graph | 50.5 | 0.996 | 1.093 |
| histogram | 450.0 | 1.000 | 1.000 |
| interp | 532.9 | 0.989 | 0.923 |
| knucleotide | 586.0 | 1.024 | 1.048 |
| kostya_bf_bench | 26722.6 | 0.944 | 0.965 |
| kostya_bf_mandel | 262600.4 | 0.955 | 1.062 |
| kostya_json | 234.5 | 1.048 | 1.119 |
| kostya_matmul | 657.2 | 1.196 | 1.392 |
| levenshtein | 6672.3 | 1.017 | 1.121 |
| lexer | 76.4 | 1.047 | 1.144 |
| list_sum | 52.5 | 1.000 | 1.000 |
| loop_sum | 2700.0 | 1.000 | 1.000 |
| loops | 1700.3 | 1.000 | 0.941 |
| lru | 427.9 | 1.086 | 1.105 |
| mandelbrot | 195.4 | 1.000 | 0.984 |
| mapwork | 63.7 | 1.077 | 1.169 |
| matmul | 323.2 | 1.002 | 1.211 |
| nbody | 322.6 | 1.036 | 1.120 |
| nqueen | 1720.3 | 0.905 | 0.953 |
| primes | 654.5 | 1.063 | 1.087 |
| resultpath | 270.0 | 1.000 | 1.000 |
| sieve | 102.7 | 1.000 | 1.000 |
| sortwork | 318.0 | 1.085 | 1.085 |
| spectralnorm | 4162.7 | 1.000 | 1.000 |
| strbuild | 20.0 | 1.200 | 1.300 |
| strsplit | 201.7 | 0.995 | 1.017 |
| structlist | 380.3 | 1.000 | 1.000 |
| structloop | 27.5 | 1.000 | 1.182 |
| switchwork | 477.4 | 1.000 | 1.000 |
| tak | 1023.7 | 1.045 | 1.071 |
| wordfreq | 182.8 | 1.012 | 1.042 |

Reading it: about 15 programs do not move at 6 registers (their loops hold fewer than 6 values); the damage is in kostya_matmul (+39%), strbuild (+30%), base64 (+28%), matmul (+21%), structloop (+18%), fasta (+18%), mapwork (+17%), lexer (+14%). Some programs get faster with fewer registers (crc32 at 6 registers -13%, nqueen at 8 registers -9.5%), so the allocator's choice of what to keep is not optimal at full size either.

Ways to speed up without registers, in the order I am doing them (the first is in PR 214):
1. Fewer instructions and live values: join repeated loads and address arithmetic (levenshtein's branch-free minimum), fold `index * 8` into every address that uses it instead of keeping the scaled value in a register.
2. Cheaper checks: a bounds or overflow test whose answer is already known on the path is removed by the optimiser, not by registers (range tests as one unsigned compare, PR 204).
3. Memory operands: `cmp r, [mem]` and `add r, [mem]` for a value used once, so a spilled operand costs no extra register and no extra instruction. The allocator already folds a use into an operand when the value is spilled; the gap is values kept alive across a loop only to be read once per pass.
4. Loads that are invariant by language rule: the data pointers and elements of a `let` parameter cannot change during the call (value semantics, no aliasing of a borrowed value), so a loop reads them once or uses them as memory operands; levenshtein still reloads `a.data`, `b.data` and `a[i]` every pass.
5. Strength reduction: a pointer that steps by 8 instead of `base + i*8` removes the index register in streaming loops (structlist, kostya_matmul, matmul); a counter that is also the address takes one register instead of two.
6. Less live state by design: rematerialise constants and `slot + k` addresses at the use instead of spilling them, and split ranges only around calls and loops where it pays.
7. Evaluate every change at 6 and 8 registers next to the full-size numbers; a change that helps only at full size is not accepted.
