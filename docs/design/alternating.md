# The alternating sequence (draft 1)

Status: decided 2026-11-08 (owner answered all eight questions in section 9); not built yet. Written at the owner's request: "I want to name it something else, not tuple, and give it its own
function and the works." **Docs only; no compiler change in this PR.** The owner chose the name **braid** (section 1).

The idea is the owner's own: a fixed-size run of N slots whose types **repeat in a cycle**. `(int, string; 3)` is an int, a
string, an int. `[(A, B); N]` is N pairs, laid out exactly like `(A, B; 2N)`. No mainstream language has this as a built-in type
(C has arrays of structs, numpy has structured arrays, Zig has `MultiArrayList`; each gets close to one use of it, none is a
type with its own operations).

## 0. What it is, and what exists today

| Spelling | Meaning |
|---|---|
| `(A, B)` | a tuple: exactly two slots, types A and B, no cycle |
| `(A, B; N)` | **braid**: N slots; slot `i` has the type of position `i mod 2` in the list. `N` is a compile-time constant |
| `(A, B, C; N)` | the same with a cycle of three (any cycle length k >= 2 works; k = 2 is the case that matters) |
| `[(A, B); N]` | an array of N tuples. Same memory as `(A, B; 2N)` (section 5), but a different type: its slots are reached as pairs |

Built today (compiler only, no library): the parser reads `(A, B; N)` (`FixedTuple` in `ast.rs`), the checker turns it into
`AltTuple` (`types.rs`), a constant position `x.3` has the type of slot `3 mod k`, and a pattern `const (a, b, c) = x`
binds slot by slot (`typecheck/pat.rs`). It appears in `examples/tour.pel` (`grid: (int, string; 4)`). There is **no literal**
(section 2), no dynamic indexing, no operations, and `lower/` has the type only. Everything below is what the owner asked for on
top of that.

## 1. The name: braid (decided)

The word must not be "tuple" (a tuple has no cycle), must not clash with a keyword or a std name, and should say
"two things taking turns". Checked against `keyword.rs` and `docs/design/keywords.md`: none of these is used there.

| Candidate | Why it fits | Cost |
|---|---|---|
| `tide` | ebb, flow, ebb, flow: the sea word for "alternates"; one syllable | reads as a time or physics word before a data one |
| `tack` | sailing: a boat zig-zags upwind, left, right, left. Also "tack on" | `tack` alone says little to a newcomer |
| `braid` | two strands crossing in turn; the picture of interleaved memory | long-ish; suggests weaving more than two |
| `weave` | verb and noun; zip, then weave, is natural (`weave(a, b)`) | a verb: reads oddly as a type |
| `ripple` | a regular wave; sea flavour | suggests repetition of one thing, not two |
| `strand` | what a braid is made of; also "a beach" | does not hint at alternation |
| `swell` | sea word; a long regular wave | same weakness as `ripple` |
| `cycle` | says exactly what the types do (cycle through A, B) | not sea; `cycle` is an iterator name elsewhere (Rust) |

**Decision (owner, 2026-11-08): `braid`.** `tide` was the runner-up. `braid` is the only one whose picture matches the memory
(two strands in turn) and it makes the operations read well: `b.strands()`, `braid.zip(xs, ys)`, `b.unbraid()`. The words `strands`/`unbraid` below are the long spellings of `evens`/`odds` and `split`; section 4 gives the table.

Where the name appears: the type's name in messages, the constructor namespace (`braid.zip(...)`, `braid.fromLists(...)`), and a
spelled-out type **`braid<A, B; N>`** that means exactly the same as `(A, B; N)` (decided: both spellings are allowed; the
formatter leaves each as written, `show` in messages prints the `braid<...>` form).

Implementation note: today `T.f()` calls on a type are not implemented in the checker (see `json.md`), so `braid.zip` and
`braid.fromLists` need that first, or ship as the free functions `braidZip` and `braidFromLists` until it exists.

## 2. Literal

An alternating sequence has no literal of its own. A parenthesised list takes its type from the place it lands in:

```
const t: (int, string; 3) = (7, "seven", 8)      // a list of 3 items where the type says 3 slots: it is a braid
const u = (7, "seven", 8)                         // no annotation: an ordinary tuple (int, string, int)
const z: (int, string; 6) = (0, ""; 6)            // repeat form: the cycle (0, "") filled up to 6 slots, like [0; 9]
```

Rules:

- **L1** The item count must equal `N` exactly, and item `i` must have the type of slot `i mod k`. A wrong count or type is a compile
  error that names the slot.
- **L2** The repeat form `(a, b; N)` in an expression repeats the cycle `a, b, a, b, ...` to `N` slots. It mirrors `[v; N]`. It is
  told apart from the type `(A, B; N)` by position (type or expression), as `[T; N]` and `[v; N]` already are.
- **L3** Without a type around it, a list is a tuple. This keeps `(a, b)` meaning one thing everywhere; the annotation is the
  switch. (Alternative: a marker such as `(a, b, a; alt)`. Not recommended: another word, and the annotation is nearly always
  there because a `const` table or a parameter states it.)

## 3. Reading and writing slots

The central rule, and the part that is Pelago's own: **a slot's type depends on its position, so a position the compiler
cannot see has no type.**

- **A1** `x[3]` or `x.3` with a **constant** position has the static type of slot `3 mod k`. A constant position `>= N` is a compile
  error. (Today's `x.3` already does this.)
- **A2** `x[i]` with a **runtime** `i` is a compile error ("slot type depends on `i`; use `x.first(p)` and `x.second(p)`"). There is one
  exception: when all types in the cycle are the same, `(int, int; 6)`, the type is just `int` and `x[i]` is accepted like `[int; 6]`
  (and the checker suggests `[int; 6]`).
- **A3** Pair access by pair number, typed and checked:

```
x.first(p)          // slot 2p,   type A      traps if p is not in 0 .. pairs
x.second(p)         // slot 2p+1, type B
x.setFirst!(p, v)   // changes slot 2p; v: A
x.setSecond!(p, v)
x.tail()            // only when N is odd: the last slot, type A; a compile error when N is even
```

- **A4** Exact semantics, as for lists: a bad `p` **stops the program** with the position and the length
  (`pair 5 out of range: 3 pairs`). Negative numbers count from the end (`x.first(-1)` is the last full pair), like `xs[-1]`
  in `collections.md`; a value below `-pairs` stops the program. No wrap-around, no silent clamp.
- **A5** Slice-like access is not offered in the first version (no `x[2..4]`): a range of slots would start on either type, and a
  view of that would need its own type. `evens`, `odds` and `pairs` (section 4) cover the real uses.

`len` is **slots** (`N`), the same word as for `[T; N]`. `x.pairs()` is **N / 2, rounded down**, a constant. For odd `N` there is one
extra slot, the tail; for even `N` there is none. Both are compile-time constants and the checker folds them.

## 4. First-class operations

Every operation is a method, so the name stays out of most code. Lengths are compile-time numbers where the type says so.

| Operation | Type | Notes |
|---|---|---|
| `x.len()` | `int` | slots, `N` |
| `x.pairs()` | `int` | full pairs, `N / 2` rounded down |
| `x.pairIter()` | `iter<(A, B)>` | `for a, b of x.pairIter()` visits each full pair with both slots typed. The tail is not visited; read it with `tail()`. The lazy `iter<T>` is the one from `iter.md` |
| `x.evens()` / `x.odds()` | `iter<A>` / `iter<B>` | all slots of one type, in order, as read-only views for one statement (second-class, like every reference). `for inout a of x.evens()` changes them in place |
| `x.split()` | `([A; ceil(N/2)], [B; N/2])` | two **owned copies**, typed. A copy, so no stride type is needed. Also written `x.unbraid()` once named |
| `braid.zip(as, bs)` | `(A, B; 2K)` | from `as: [A; K]` and `bs: [B; K]`. Equal lengths are checked at compile time for arrays. The inverse of `split` for even `N` |
| `braid.fromLists(as, bs)` | `result<(A, B; N), zip_error>` | from lists; `N` comes from the expected type. A length that is not `N/2` is a `zip_error`, not a stop, because list lengths are data. (Convention in this repo: data problems are `result`, programmer errors stop) |
| `x.mapPairs(f)` | `(C, D; N)` | `f: fn(A, B) -> (C, D)`; one new pair per old pair; for odd `N` the tail is kept unchanged (`C = A`), otherwise the checker says so |
| `x.mapEvens(f)` / `x.mapOdds(f)` | `(A2, B; N)` / `(A, B2; N)` | change the type of one strand |
| `x.swap()` | `(B, A; N)` | even `N` only: swaps the roles, as a copy |
| `x == y`, `x < y` | `bool` | slot by slot in order, lexicographic; both need the same type. Hash and `show` follow the same rule |
| `x.toArray()` | `[T; N]` | only when all slots have one type |

Value semantics apply as for `[T; N]`: assignment copies (or moves when the slots own something), `sink` consumes, a slot
that owns a `string` is dropped when the whole value goes out of scope. Passing it as `let` gives a view, as `inout`
lets the callee change slots, as for arrays.

Not offered, on purpose: `push` (the size is part of the type), `sort` (the slots do not share an order), `reverse` of an odd
`N` (it would change which strand is which; use `swap` or `split`), `concat` (the types of the second part would have to
continue the cycle, which is easy to get wrong).

### Example

```
// an (opcode, operand) program in flash: 64 slots, 32 instructions
const prog: (uint8, uint16; 64) = (0, 0; 64) with { ... }      // table written out in the real file

var pc = 0
while pc < prog.pairs() {
    switch prog.first(pc) {
        case 1: acc += int(prog.second(pc))
        case 2: acc -= int(prog.second(pc))
        default: stop("bad opcode")
    }
    pc += 1
}

for op, arg of prog.pairIter() { print("#op #arg") }
const (ops, args) = prog.split()          // [uint8; 32] and [uint16; 32]
const back = braid.zip(ops, args)          // the same 64 slots
```

(The `with { ... }` is only a sketch of "a long literal written out"; the real literal is the list of section 2.)

## 5. Memory layout

**Interleaved, in declaration order, no reordering.** The value is laid out as if it were a struct with N fields, field `i` of the
type of slot `i mod k`, each at its natural alignment, in order, with padding between and at the end. `sizeof` is a constant.

- `(int, string; 4)`: int, string, int, string, one after the other.
- `(uint8, uint16; 64)`: `uint8` at 0, `uint16` at 2 (one byte of padding), next `uint8` at 4; 4 bytes per pair, 128 bytes in all.
  The tool `--stats` prints the padding. Where flash size matters, use `split()` on a `const` once and keep two dense arrays;
  or put equal-sized types in the cycle.
- **`[(A, B); N]` has the same layout as `(A, B; 2N)`.** This holds for every A and B, and the proof is short: slot `B` starts at an
  offset that is a multiple of `align(B)` and `size(B)` is a multiple of `align(B)`, so the end of `B` is a multiple of `align(B)`;
  the next `A` starts at that end rounded up to `align(A)`, and a pair struct is padded to `max(align(A), align(B))`, which rounds the
  same end up to the same place (if `align(A) >= align(B)` both round to `align(A)`; otherwise the end is already a multiple of
  `align(B)`, which is a multiple of `align(A)`). So the conversion between the two types costs nothing: `x.asPairs()` and `y.asSlots()`
  are reinterpretations, no copying.
- Relation to `soa<T>` (`memory.md`): `soa<(A, B)>` would store **one array per field** (all `A`s together, all `B`s together).
  braid stores **one array interleaved** (A, B, A, B). They are opposite ends of the same choice: use braid when the two are read
  together (opcode with operand, key with value, a coordinate with its id); use `soa` when one field is scanned alone.
  `x.evens()` on a braid is a strided walk, which is the price of the interleaved side.
- ABI: a braid passed to a C function (`ext fn`) is passed as a pointer to this layout. Nothing else about it is promised to C.

## 6. JSON and regex

### JSON (`docs/design/json.md`)

`json.md` lists tuples as not decodable and not encodable yet. Proposal for both tuples and braid at once, because the rule is the same:

- **Encode:** a JSON array with one element per slot: `(int, string; 3)` is `[7, "seven", 8]`. A `[(A, B); N]` is an array of
  N two-element arrays: `[[7, "seven"], [8, "eight"]]`. The two types have the same memory but **different JSON**, and that is
  deliberate: the type says which one the author means.
- **Decode:** the array must have exactly `N` elements (a tuple: its own count). Slot `i` decodes as the type of slot `i mod k`. Errors
  keep the path style of `json.md`: `$[2]: expected string, found number` and `$: expected 3 elements, found 4`.
- It decodes with `@derive(fromJson)` as a field of a struct, exactly as `[T; N]` would. No attribute needed on the type itself.
- Objects stay `map<string, V>` or a struct. braid is for positional data.

### Regex typed captures (planned, Build card)

A regex literal compiles to code with **typed named captures as struct fields**. braid fits in two places, and no more:

1. **A group repeated a fixed number of times**: `(\d+)=(\w+);` written three times, or `{3}` on a group of two captures, gives
   `(int, string; 6)`, with the cycle being the group's captures in order. Bounded and known at compile time, like the
   regex engine's memory.
2. **An unbounded repeat** (`(?:(\d+)=(\w+);)*`) gives a **list of pairs** `[(int, string)]`, not braid, because braid's size is fixed.
   `braid.fromLists` or `x.split()`/`braid.zip` convert when the count turns out to be right.

Open for the regex card: whether captures keep the position-in-cycle names (`k`, `v`) when named groups are used. Recommendation: named
groups always become struct fields, braid only for unnamed repeated groups.

## 7. What is conventional, and what is Pelago's own

**Conventional (say so, do not dress it up):**

- Interleaved storage of mixed data: array of structs, numpy structured arrays and complex numbers stored re, im, re, im, BLAS strides.
- `zip`, `unzip`/`split`, `map`, pair iteration, `first`/`second`: names from Python, Haskell and Rust.
- Constant-position access with a static type: Rust and Swift tuples (`x.0`).
- Bounds that stop the program, negative indexes from the end: already Pelago's list rule.

**Pelago's own:**

- A **type-level cycle**: `N` slots whose types repeat, so a flat table of two kinds of thing is one value with one size and no
  struct, and the checker knows every slot's type from its position.
- **No dynamic `x[i]`** on a mixed cycle: the type depends on `i`, so the language gives you `first(p)` and `second(p)` instead of
  a union or a cast. (Most languages would return a variant or a pointer here.)
- The **proved equal layout** of `[(A, B); N]` and `(A, B; 2N)`, so you can pick the view per function at no cost.
- Pair and strand operations designed for it from the start, on top of `iter<T>` and the exact-trap rule (no wraparound, no clamping).
- Fits Pelago's standing goals: fixed size and known layout suit microcontroller tables in flash and need no allocation, and nothing
  here needs a runtime.

## 8. Fixes made to other docs in this change

The idea was worded three ways: "fixed-size tuple" (`keywords.md`, `memory.md`), "your alternating primitive" (`collections.md`). All
three now say **braid** and link here. `keywords.md` also said "Fixed-size tuple"; a tuple has
no cycle, which was the source of the confusion. No code or rule changed.

## 9. Decisions (owner's answers, 2026-11-08)

1. **Name:** `braid`.
2. **Type spelling:** both `(A, B; N)` and `braid<A, B; N>` are allowed.
3. **Literal:** the type annotation decides tuple versus braid; no marker word.
4. **No runtime `x[i]`** on a mixed cycle; `first(p)` / `second(p)` instead (a same-type cycle accepts `x[i]`).
5. **Cycle length:** longer cycles `(A, B, C; N)` are allowed in the type; only `first`/`second` are provided until someone needs more.
6. **Odd `N`** is allowed, with `tail()`.
7. **JSON:** one flat array per braid; `[(A, B); N]` is an array of two-element arrays.
8. **Padding:** natural alignment, padding shown in `--stats`; a packed variant may come later for flash tables.

9. **Long spellings stay** (owner, 2026-11-08): `unbraid` is an alias of `split`, `strands` of `evens`/`odds` taken together, alongside the short names.
10. **One card** covers the whole feature, all steps in one go (parser literal and `braid<...>` spelling, checker, lowering, library functions, JSON, tests, docs); the owner asked the orchestrator to create it.

Still open (small, for the build card): whether `braid.zip` waits for `T.f()` calls or ships first as `braidZip`.
