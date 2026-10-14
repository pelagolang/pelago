# Layout and ownership extras (draft 2)

Two questions about memory that the earlier pages left open: array-of-structs versus
struct-of-arrays, and linear types.

## 1. Where we are

Values are owned. A `sink` parameter or a `move` takes a value away, `deinit` runs when a value goes
out of scope, and a moved value cannot be used again. In type-theory terms that is **affine**: a value
may be used at most once, and it may also be dropped.

## 2. AoS and SoA

- **AoS** (array of structs): each element's fields sit together. This is how C lays out `point[]`, and it
  is what `[point]` is in this language. It is the right layout when you use whole elements, and it matches C
  for interop.
- **SoA** (struct of arrays): one array per field. It is the right layout when a loop touches one field
  (all the `x` values) because the data is dense in the cache and vectorises well.

**Proposal.** The default is AoS, because it is predictable and C-compatible. SoA is opt-in as a collection
type with the same interface:

```
var points: soa<point> = soa []
points.push!(point { x: 1.0, y: 2.0 })

points[0].x                  // reads through a view of element 0, same code as a list
points.x                     // the whole x column as a read-only [float]: a SIMD-friendly slice
for inout p of points { p.x += 1.0 }
```

Because references are temporary (second-class) under value semantics, `points[i]` is always a view
that exists for one statement, so the layout behind it can change without changing any code that uses it.
That is a real advantage over languages where element references can escape. `soa<point, 8>` is the blocked
variant (AoSoA) that groups 8 elements per block for vector widths.

A compiler that picks the layout by itself stays on the "later" list. It needs whole-program analysis.

The fixed-size tuple `(A, B; N)` is an interleaved layout, the AoS flavour of mixed types. `soa<T>` is its
opposite, so the two cover both ends.

## 3. Linear types

Affine types let a value be dropped silently. **Linear** types forbid that: a value must be consumed
exactly once. That is how you make "this file must be closed", "this transaction must be committed or rolled
back" or "this permission must be handed back" into compile errors. Rust has no linear types (only warnings).
Austral and Hylo do.

**Proposal.** We almost have it already. Add an opt-in modifier:

```
linear struct transaction { ... }

fn commit(sink t: transaction) -> void ! db_error { ... }
fn rollback(sink t: transaction) { ... }

fn work() -> void ! db_error {
    const t = begin()?
    do_things(t)               // read-only use, fine
    t.commit!!()               // consumed. Forgetting this is a compile error.
}
```

- A linear value must be consumed on every path, including early returns and `?`. The compiler says which path forgot.
- A linear type has no `deinit`, because it can never be dropped implicitly.
- `sink` already means "takes the value away", and `!!` already marks the consuming call, so no new call-site syntax.
- Generic code needs care: by default type parameters may be dropped, so `T` can't be a linear type unless the
  bound says so (`<T: linear>`). Keep this door open when the type checker is designed; do not hard-code
  "every type can be dropped" anywhere.

**Staging.** Version 1 is affine plus a `@must_use` warning. Real `linear` types arrive after the type checker
exists, because they change the rules for generics. The word `linear` is reserved now (soft keyword).

---

## Built: soa

`soa<T>` for a struct `T` works as described above (see IR24 in `ir.md`): `soa []`, `soa [a, b]`, `push!`, `pop!`, `insert!`, `remove!`, `clear!`, `len`, `points[i]`, `points[i].x`, `points.x`, `for` and `for inout` loops, `copy`, `==`, `print`. The blocked form `soa<T, N>` is accepted and behaves the same; its blocked memory layout is left for the backend.

## Temporaries

A value that is made but never named (a function result you ignore, the object behind `make_dog("a").name`, an argument built on the spot) is destroyed at the **end of its statement**, in the reverse order it was made. If something takes it (a variable, a field, a `sink` parameter, `return`) the new owner destroys it later instead. The compiler keeps these books while it builds the code (the ledger, IR22); there is no run-time cost in straight-line code beyond what variables already pay.

## Decisions from draft 1

- **M1** AoS is the default layout. `soa<T>` is the opt-in collection, with `soa<T, 8>` as the blocked variant.
- **M2** Column access (`points.x`, a read-only `[float]`) is in.
- **M3** Compiler-chosen layouts stay a later optimisation.
- **M4** `linear` is an opt-in modifier, staged after the type checker. The word is reserved now as a soft keyword.
