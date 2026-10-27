# Bump arenas (draft 1)

An arena is a block of memory where allocating is a pointer bump and freeing everything in it is one store. This page says
what that means for a language with owned values, `deinit` at scope end and no libc. Card: [A-13](../plans/cards/A-13-arena.md).
Source of the idea: the allocator in Goose; what we take is the cost model, not its rules.

## 1. Why

Today every `box(...)`, string and list buffer goes through `rt_alloc`: a size class, a free-list pop, zeroing on reuse, and the
same again on `rt_free`. `bintrees` builds and drops a tree of 131071 nodes twenty times; each node is one alloc and one free
plus the recursive drop. With an arena a node is `top += size`, and dropping the tree is `top = 0`.

What an arena does **not** fix, so nobody expects it to:

| Row | Real cause | Does an arena help? |
|---|---|---|
| `bintrees` | one box per node, recursive drop | **Yes**: the target use |
| `strsplit` | each piece is an `str` view, plus the joined string growing | Little. The growing `string` is one buffer (`rt_realloc`); the pieces need no allocation. Look for temporaries first |
| `lexer` | `str.chars()` builds a whole `[char]` before the loop | **No**: the fix is a lazy `iter<char>` that decodes as it goes. Stage AR5 does that; an arena would only make the wasted list cheaper |
| `mapwork` | map storage (`rt_alloc` of tables, rehash) | **No**: nothing short-lived. S-01 (own map storage) is the lever |

So the plan has one stage that is really about arenas (AR2 to AR4) and one that uses the same idea inside the library (AR5), and the
card says plainly which rows move.

## 2. The model

An `arena` is an **owned value** like any other (`sink` to move it, `deinit` frees it). It owns one growable block. Values placed in
it are named by a handle, `aref<T>`, and read through the arena:

```
var a = arena()
const n = a.new!(node { value: 1, left: none, right: none })   // aref<node>
a.get(n).value                                                  // a bit-for-bit copy (plain types)
a.set!(n, node { ... })
a.reset!()                                                      // every aref made so far is now stale
```

Rules, all from rules the language already has:

- **No lifetimes to check.** A handle is a plain 8-byte value: 32 bits of byte offset from the arena's base, 31 bits of epoch. It is not
  a pointer, so growing the block (`rt_realloc`, which on Linux moves pages without copying) cannot break it.
- **`get` checks the epoch** (one load and one compare, predicted). A stale handle (after `reset!`, or from another arena) stops the program
  with `used a handle after the arena was reset`. Safe in release builds, no new checker rule, the same stance as the exact overflow traps.
- **`get` returns a copy.** Plain types copy bit for bit, so no view is needed and nothing can point into a block that moves. `new!`, `set!` and
  `reset!` take the arena `inout`; `grow!` may move the block, which only handles (offsets) survive.
- **The arena can be moved** (sink, sent to a task) with handles alive; offsets do not care. It cannot be shared: one owner, as always.
- **Epochs** come from a counter bumped at `arena()` and at every `reset!`, so two arenas do not accept each other's handles (except after
  2^32 bumps; a wrap is documented, not guarded). With real threads (A-12 C6) the counter becomes an atomic.
- **Limit.** 4 GiB per arena (the 32-bit offset). Over it stops with `arena full`, like out of memory. A wider handle is a later choice
  (Q3), not a v1 need.

## 3. Ownership and `deinit`

This is the crux: an arena frees by forgetting, so anything in it that needs `deinit` would leak or double-drop.

- **v1: only plain types.** `a.new(v)` requires `T` to need no drop (numbers, `bool`, `char`, structs and enums of those, other `aref`s).
  A type with a `string`, `list`, `box` or a user `deinit` stops the program (`an arena holds only types without drop glue`; making it a compile error is a checker follow-up): `arena.new` needs a type without drop glue; use `box`
  or give the arena a drop list (AR6). The check is the lowering's existing `needs_drop(ty)`, exposed to the library as `rt_needs_drop<T>()` (a constant per type, so the optimiser removes the test).
- **An `aref<T>` is plain**: copying it copies the handle, dropping it does nothing. Two handles to one value are fine because `get` is a
  view and `set!` needs the arena `inout`; there is no way to hold two writers.
- **v2 (AR6, optional): a drop list.** For a type with drop glue, `new` also links a 16-byte record (drop function, offset) into a list kept
  in the arena; `reset!` and `deinit` walk it in reverse order of creation, then bump. Cost is paid only by arenas that hold such values;
  plain-only arenas keep the one-store reset. The reverse order matches how scope-end drops run today (the "Temporaries" rule in `memory.md`).
- **A panic stops the program** (runtime decisions of 2026-10-27): no unwinding, so no half-dropped arena is observable.
- **The leak check** sees the arena's block as one live allocation until `deinit`; a forgotten arena is reported like any leaked block.

The arena's own `deinit` is one `rt_free`.

## 4. The runtime boundary

The arena is a **library type** (`compiler/core/core/arena.pel`) written over `rt_alloc`, `rt_realloc`, `rt_free` and the load/store
intrinsics. It adds no IR op and no native routine. Consequences:

- It runs unchanged in the interpreter, natively, on every target.
- The runtime period (no libc, own OS layer) only has to keep the contract of those three routines; the arena never calls the OS.
  If that period wants arenas to reserve address space up front (so growth never moves anything), that is a change inside `rt_alloc`'s large-block path, not here.
- The first bump is inlinable: `top`, `cap` and `base` sit in the arena header, so the optimiser can turn `new` into add, compare, store.
  Whether the native backend keeps them in registers across a loop is a later A-11 item, not a promise.
- Memory from `new` is **not zeroed** (it is always written by `new` before anyone reads it); a fresh block is zero anyway.

## 5. What this does not do

- No hidden arena and no `region { }` block that changes where `box` allocates. The reasons: a value that escapes the block would dangle,
  and the checker would need lifetimes, which the language chose not to have. Handles with an epoch give the same speed for explicit use.
- No mark/release stack (free back to a mark). It would need an epoch per mark to stay safe; Q2.
- No sharing between tasks, no free lists inside the arena, no per-value free.

## 6. Stages

| Stage | What | Tests | Rows |
|---|---|---|---|
| AR1 | This page, card A-13, decisions row | docs only | none |
| AR2 | `arena`, `aref<T>`, `new`, `get`, `set!`, `reset!`, `used`, `deinit`, `rt_needs_drop<T>()`; epoch check | interpreter and native: stale handle stops, move keeps handles, growth keeps handles, leak check clean, a type with drop glue is rejected | none yet |
| AR3 | `bintrees` twin written with an arena (`arena_bintrees` under `bench/programs/`, same output) | one run of each, no suite | `bintrees` |
| AR4 | Optimiser pass: inline `new`/`get` fast paths, keep header fields in registers in loops | IR tests; one before/after run | `bintrees` |
| AR5 | Lazy `str.chars()`, no list; the short-lived buffers in `text.pel`/`objects.pel` use a function-local arena or stack slot | existing tests; `lexer` once | `lexer` |
| AR6 | Drop lists for non-plain `T` (optional, Q1) | drop order, reset runs drops | none |

Each stage is its own PR on its own branch. No stage runs the full benchmark suite; each names the single row it moves.

## 7. Questions for the owner

1. **Plain types only in v1, drop list later (AR6)?** Recommend yes: it keeps `reset!` one store and the rule easy to explain.
2. **Mark/release?** Recommend not in v1. Scoped scratch is the one use for it (parse, then throw away) and a `reset!` of a short-lived arena does the same.
3. **Handle width.** Recommend 32+31 now (4 GiB per arena). Alternative: 16 bytes (pointer plus epoch) with no size limit, slower to store in trees.
4. **Names.** `arena`, `aref<T>`, `a.new(v)`, `a.get(r)`. Alternatives: `a.alloc(v)`, `region`. Recommend as written; `new` is not a keyword here.
5. **Should `box(x)` ever pick an arena by itself** (an ambient arena in a scope)? Recommend no, see section 5.
