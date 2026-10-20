# Vectoriser (card A-11)

Dennis asked for it (2026-10-20) after unrolling brought `structloop` to Rust's time (`ir/opt/unroll.rs`). This page is the plan: what the first version does, what it leaves alone, and the order of the pull requests. Rust's own code for `structloop` is the model: counters kept as steps, four iterations per pass in two SSE2 registers.

## 1. What we can count on

Baseline x86-64 has SSE2 only. A register holds two 64-bit lanes (or four 32-bit, eight 16-bit, sixteen 8-bit). There is no 64-bit multiply (`pmuludq` is 32 by 32), no signed 64-bit compare (`pcmpgtq` is SSE4.2), no gather. So 64-bit integer loops gain at most 2x in the instructions they run, and loops bound by memory or by a chain gain nothing. Expect gains on: sums and bit tricks over counters (the `structloop` family), element-wise `+ - & | ^ << >>` over arrays of `int`, and byte or 32-bit loops where the lane count is 4 to 16. Do not expect gains on float reductions (adding in another order changes the result, and we keep exact semantics), on stores to one cache line (`sieve`) or on division-heavy loops.

## 2. Rules the vector code must keep

- **Same results and same stops.** A checked operation stops in the same pass as the scalar loop would. Vector code never does a checked operation per lane; instead a guard in front of the vector loop shows that no lane can overflow, and otherwise the scalar loop runs. For a sum of non-negative terms with a known bound (a `& 1023`, a byte) the guard is `acc + passes * bound` fits; for array elements the guard is a bound taken from the types, or the loop stays scalar.
- **Bounds checks.** A vector load covers `K` elements at once, so the vector loop is entered only when `i + K <= len` holds for all of its passes (one test in the guard); the scalar remainder, which still has the per-element checks, runs the last passes and stops at the exact element.
- **Wrapping operations only inside the vector loop**, since they are the only ones with lane-wise instructions. A checked operation the guard has proved safe is a wrapping one there.
- **Size.** The vector loop is one copy of the body per unroll step plus the original loop as the remainder; same budget idea as `unroll.rs` (`K * size` under a cap, only for loops that run long).

## 3. Shape of the work

1. **Vector values in the IR** (PR 1, done): a type `v2i64` and the operations `vsplat`, `vbuild a, b` (lane offsets such as `[0, 1]` are built from scalars, so there is no `vconst`), `vadd`, `vsub`, `vand`, `vor`, `vxor`, `vshl k`, `vushr k`, `vreduce_add`; a vector is loaded and stored with the ordinary `load v2i64` and `store` (unaligned, 16 bytes). The interpreter defines each lane by lane, the verifier checks types, the printer shows them. Native: encodings in `x86.rs` with golden bytes (`paddq`, `psubq`, `pand`, `por`, `pxor`, `psllq`, `psrlq`, `punpcklqdq`, `pshufd`, `movups`, `movq`), vectors are allocated in the `xmm` class the floats use, and block-parameter cycles park a whole register (16 bytes). A vector has no memory home: one that would live across a call, or that finds no register, is a compile error in the native generator (not a wrong program), so the vectoriser must only make vectors in call-free loops. A vector never leaves the loop except through `vreduce_add` (or a store).
2. **Reductions and counters** (PR 2): the vectoriser proper for two-block counted loops (the shape `unroll.rs` takes): loop-carried counters become a vector of lane offsets plus a splatted step; an accumulator `acc += f(counters)` becomes a vector accumulator reduced after the loop; the guard, the vector loop and the scalar remainder. Target: `structloop`.
3. **Element-wise array loops** (PR 3): `vload`/`vstore` for `int` arrays with the hoisted bounds guard; `list_sum`-style reductions over a list. Needs the element type and stride to be visible in the IR (today a list is a pointer, length and a load at `ptr + i * 8`).
4. **Narrow lanes** (later, only if a benchmark wants it): bytes and 32-bit values with 16 or 4 lanes.

Each step lands with: differential tests (interpreter, optimised IR, native on Linux and Windows), a case per boundary of the guard, the benchmark table, and the generated-code size next to it. A step that does not make a benchmark faster or smaller is not merged on its own.

## 4. Open questions

1. Is `K = 4` iterations per pass (two registers, like Rust) worth the extra size over `K = 2`? Decide by measuring on `structloop` in PR 2.
2. Should AVX2 (4 lanes) be a later option behind a CPU check at start-up? It needs a runtime dispatch or a build flag; not in version 1.
