Id: A-06
Title: `soa<T>`: struct-of-arrays collection
Column: Done (recommended answers used; blocked layout deferred) (design decided in `memory.md` M1 to M3; build plan here)
Order: 6
Depends on: A-02 (box is not needed, but recursive types are), A-04 (the optimiser makes columns fast), A-05 stage M2 for `soa<T, N>`
Size: L
Touches: types, checker, lowering, `core.pel`, docs

# 1. What and why

`[point]` stores whole points one after another (array of structs, AoS). A loop that only touches `x` still drags every `y` through the cache. `soa<point>` stores one array per field:
all the `x` values together, all the `y` values together. Loops over one field become sequential memory scans, which is much faster on modern CPUs and vectorises, and it is the layout games, simulations and
numeric code want. It is on the language's performance list as an explicit, opt-in collection (decision M1). It was not lowered because it needs layout work, and `meta N` for the blocked form.

# 2. Decided behaviour (from `memory.md`)

```
var points: soa<point> = soa []
points.push!(point { x: 1.0, y: 2.0 })

points[0].x                  // a view of element 0, same code as a list
points.x                     // the whole x column as a read-only [float]
for inout p of points { p.x += 1.0 }
```

- The interface matches a list: `len`, `push!`, `pop!`, `insert!`, `remove!`, `[i]`, `for` loops, `clear!`, `copy`, `==`, printing (as a list of structs).
- `points[i]` is a **view** that lives for one statement (views are second-class), so the layout behind it can differ without changing any caller code.
- `points.x` is a read-only slice of one column. `inout` column access (`points.x[i] = ...`) is allowed through the element path.
- `soa<T, 8>` is the **blocked** layout (AoSoA): groups of 8 elements, each group holding 8 values of each field.

# 3. Representation

For `soa<T>` with fields `f1: A, f2: B, ...`:

- Header: `len`, `cap`, then one data pointer per field (`n + 2` words). All columns share one capacity.
- Column `k` is a contiguous buffer of `cap` values of the field's type.
- One allocation per column on growth (simple), or one allocation for all columns with computed offsets (faster to free, and keeps columns close; do this as an optimisation after the simple version works).
- Fields that are themselves owning types (strings...) are fine: a column of strings is an array of string headers; drop walks each column.
- **A struct with a single field** or a **tuple** `(A, B)` also works (a column per element).
- `soa<T>` requires `T` to be a struct or a tuple (not an enum, `obj`, or generic parameter), checked with a clear error.

Blocked `soa<T, N>`: element `i` lives in block `i / N`, lane `i % N`; each block is `N * size_of<f1>` bytes of `f1`, then `N` of `f2`, and so on. `N` must be a power of two (check); `i / N` and `i % N` become shifts and masks.

# 4. Work

1. **Types** (`types.rs`, resolver): `soa<T>` and `soa<T, N>` as built-in type constructors (keyword exists). `TyKind::Soa(Ty, ArrayLen)`.
2. **Checker**: element type rules; methods declared in `core.pel` as `impl<T> soa<T>`; literal `soa []` and `soa [a, b]`; indexing yields `T` view; `points.x` column access = field lookup on the element struct returns `[F]` view; `for` patterns bind element views; `inout` loops.
3. **Value pass**: `points[i].x = v` is an `inout` use of `points`; `points.x` is a read use of the whole collection (cannot be changed while a column view lives); exclusivity as for lists.
4. **Lowering**:
   - layout and header; `push`/`pop`/`insert`/`remove` written in `core.pel` over `rt_*` intrinsics generalised by a **field-loop**: the lowering unrolls "for each field" while generating the instance (this needs a small new intrinsic form, `rt_soa_each_field<T>(...)`, or generate the instance bodies in Rust like the glue functions; **recommend glue-style generation in Rust**, since the library language has no reflection yet);
   - element view: a pseudo-place `(soa ptr, index)`; `points[i].x` becomes `column_x[i]` directly (no struct is built). When a whole element is needed as a value (`const p = points[i]`), build it by loading each field.
   - drop, clone, eq, show glue.
5. **`for` loops** over `soa<T>`: iterate the index, bind `p` as an element place; `p.x` compiles to `column_x[i]`.
6. **Optimiser interaction (A-04)**: columns are plain buffers, so loops over one column are ordinary strided loads; promote loop-invariant column base pointers into registers (a free result of promotion + loop-invariant motion later).
7. **Tests**: push/pop/insert/remove/get/set for a two-field struct, three-field with a string, a tuple; `points.x` slice equals the list of x values; `for inout` update; `copy`, `==`, print; blocked `soa<point, 4>` with 10 elements (spans three blocks); error messages for enums and non-power-of-two `N`; leak checks on all.
8. **Benchmark** (interpreter step count is not meaningful for cache effects, so keep this as a backend-era test): record the expected speed-up when the backend exists.

# Done means

- All behaviour in section 2 works for `soa<T>`; blocked form works after A-05 M2 lands (until then it can take only literal `N`).
- Docs: `memory.md` updated with "built" notes and the layout; `ir.md` gets IR24 (soa header and column layout); `collections.md` mentions `soa` operations; `ROADMAP.md` line removed.
- fmt, clippy, tests green.

# Questions for the owner

1. Single allocation for all columns from the start, or one per column first (simpler, then optimise)? **Recommendation: one per column first.**
2. Should `points.x` allow `inout` column access (`for inout v of points.x`)? **Recommendation: yes after the value pass can express "inout of one column while others are untouched"; start with read-only.**
3. Allow `soa` of enums later through a tag column plus payload columns? **Recommendation: no, not in version 1.**
