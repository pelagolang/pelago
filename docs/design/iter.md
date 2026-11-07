# Lazy iterators by fusion (draft 1)

What `iter<T>` is today, what `collections.md` already decided it should be, and the smallest way to get there.
Card: [A-15](../plans/cards/A-15-iter-fusion.md). Follows the arena work (A-13), which found `lexer` waiting on an eager `chars()`.

## 1. Today

`iter<T>` is the same buffer as `list<T>`. Every producer (`chars`, `bytes`, `split`, `range.iter`, `keys`, `values`, `set.iter`)
builds a whole list; every adapter (`map`, `filter`) builds another. A `for` over the result then walks the last list.
Cost: one allocation and one pass per stage, and memory proportional to the input even when the loop stops at the first match.

## 2. What was decided

`collections.md` section 8: "Chains are lazy and the compiler fuses them into one loop (no temporary lists). An iterator is a
temporary view, so it cannot be stored or returned." So the language promise is already fusion; the implementation is the eager
stand-in. This page does not change the promise, only builds it.

## 3. Options

| Option | What | Cost |
|---|---|---|
| A. Fuse in lowering | Recognise `for x of <source>(.map(f) / .filter(p))*`, emit one loop that pulls from the source in place (as `for c of s.chars()` already does, PR 163) | No new representation. Each source needs a small "next" helper in the library; adapters become inline code. Non-loop uses stay eager until their stage |
| B. Real lazy value | `iter<T>` becomes (state, next function, drop function) | An indirect call per element (slower than today for small inputs), new glue for drop, `copy`, `==`, `show`, indexing, builders, and the "cannot be stored" rule still has to be enforced |
| C. Both | A for the common loops, B only for generators (`yield`) later | B stays deferred until generators exist |

**Recommendation: A now, C later.** It matches the decided design, keeps the eager `iter` for the places that really want a list
(`toList`, `sorted`, indexing), and costs nothing where no chain is involved.

## 4. Rules for fusion

- A fused chain runs **element by element**: source yields `x`, `map` closure runs, `filter` closure runs, the body runs, next `x`.
  Today all closures run over every element before the body sees the first. The two orders differ only when a closure has a side
  effect (a `print`, a counter in a captured `var`). The fused order is the one "lazy" promises; closures with effects are discouraged
  but legal, so this is a visible, documented change (Q1).
- `break` / `return` / `?` in the body stop the source: no further elements are decoded or mapped.
- Exact semantics otherwise unchanged: overflow traps in closures and bodies, `none`/`result` handling, `continue`.
- Ownership: sources that yield views (`split` gives `str` slices of the receiver) keep the receiver alive for the loop; sources that
  yield values (`chars`, `bytes`, ranges) copy plain values; owned elements (`list<string>.iter()`) are cloned as today.
- Anything not recognised falls back to today's eager path, so every program keeps working. Fusion is an optimisation with one
  documented ordering effect.

## 5. How chars, split, bytes, map, filter use it

The library keeps one decoder per source, shared by the eager and fused paths so there is a single definition:

| Source | Helper (library) | Per-turn state |
|---|---|---|
| `s.chars()` | `rt_char_at(p, i, n)` (done) | byte offset; width in a slot |
| `s.bytes()` | a plain byte load | byte offset |
| `s.split(sep)` | `rt_split_next(s, sep, pos)` gives the next piece's start and end, or "done" | position of the next search |
| `r.iter()` (range), `xs.iter()`, `m.keys()`, `m.values()` | already loop shapes in `for_each` / `for_range` | index |

Adapters inside a fused loop: `map(f)` evaluates `f(x)` into the loop variable's slot; `filter(p)` branches to the step block when `p(x)` is false.

## 6. Effect on the rows (what to expect, honestly)

| Row | Uses | Effect |
|---|---|---|
| `lexer` | `for c of s.chars()` | done in PR 163 (about 0.021 s to 0.016 s) |
| `strsplit` | `for w of text.split(" ")`, 300000 pieces | removes the 300000-entry list of 16-byte views and its growth; expected a modest gain, measure once |
| `wordfreq` | no iterator at all (string building and a map) | **no effect**: its cost is map and string work, not `iter` |
| `kostya_json`, `kostya_bf_*` | `for c of s.chars()` | already covered by PR 163 |

## 7. Stages

| Stage | What | Tests |
|---|---|---|
| IT1 | `for c of s.chars()` fused (done, PR 163) | interpreter, optimiser, native |
| IT2 | `split` and `bytes` sources in `for` | same three, plus `break`, empty separator, empty text, leak check |
| IT3 | `map` / `filter` on a fused source in `for`, with the ordering rule above | order test with a side-effecting closure; `break` stops the source |
| IT4 | Fused consumers: `sum`, `count`, `each`, `toList` over a chain | same as IT3 |
| IT5 | Real lazy value for generators (`yield`), only when generators are designed | later |

One PR per stage; no full benchmark suites, one row measured per stage.

## 8. Questions for the owner

1. **Evaluation order of side-effecting closures** changes to element by element (the documented "lazy" order). Recommend yes.
2. **Stop at IT3 or go on to IT4?** Recommend IT3 first, IT4 only if a measured row needs it.
3. **Generators (`yield`) soon?** If yes, IT5 becomes the real lazy value and `iter` stops being a list alias.
