# Bounds-check elimination: resume notes (paused 2026-10-27)

No code written yet. Findings and the design, so the thread can resume without re-reading.

## Findings
- `ranges.rs` already drops `i <u n` after the guard `i < n` and checks whose ranges are known.
- `loops` bench: no bounds checks left; the cost is `rem j, u` (variable divisor) and a load/store of `a[i]` every pass. Strength reduction of `j % u` (r += 1; if r == u { r = 0 }) is the win there.
- `levenshtein` inner loop keeps 6 checks per pass: `prev[j]`, `prev[j+1]`, `cur[j]`, `cur[j+1]` (lengths are SSA values outside the loop) and `a[i]`, `b[j]` (lengths are loads inside the loop, not hoisted because the loop stores into `cur`).

## Design (new pass `ir/opt/bce.rs`, run in `passes` before vectorise when `divs`)
Loop versioning, exact semantics:
- Innermost loops only, header test `lt i, n` (n invariant), latch sends `i + 1`, body under a size cap.
- Checks: branch to `stop "index out of range"` on `uge x, L`; x = i + k or invariant; L invariant or a load from an invariant address.
- Hoist those length loads only when every store in the loop provably cannot alias the header (guard compares addresses); calls, memcopy, realloc and free make the loop opaque.
- Preheader guard: `0 <= start`, `n` bounded, per check `n - 1 + k <u L`. The loop is cloned: the fast copy has the checks removed, the original loop is the fallback (marked `Block.slow`).
- `@checkfree fn` (attribute syntax already parses): after optimisation the build fails if a non-slow loop block still branches to an index-out-of-range stop. Needs a `checkfree` flag on `Func`.
- Strength reduction: x86 already folds `[base + i*8]`; W9 dropped it on purpose. Measure before adding. Overflow traps untouched.
