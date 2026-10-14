Id: A-02
Title: `box<T>`: make it, read it, move out of it
Column: Needs decision (6 numbered questions at the end)
Order: 2
Depends on: A-01 (small), nothing else
Size: L
Touches: resolver, checker (expressions, calls, types), value pass, lowering, `core.lang`, docs

# 1. Why this comes first

`box<T>` is the only way to write a type that contains itself: trees, linked lists, expression syntax trees (the
compiler's own syntax tree must be one, because the compiler is meant to compile itself). Today the type exists, but a program cannot
make a box, look inside one, or take a value out of one. `box(x)` stops with "`box` cannot be called", and `b.get()` does not exist.
Without it the language cannot describe its own compiler. It is also the base for returning closures (`box<fn ...>`, rule TC4).

The lowering already has half of it ready: `box<T>` is a scalar pointer (`Repr::Scalar(Ptr)`), `needs_drop` is true, there is a `Box` arm in
the equality and printing glue. What is missing is the checker side and a handful of lowering pieces.

# 2. What a box is, in this language's terms

A box is a **single-owner pointer to one `T` on the heap**. It follows the same rules as every owning value:

- It **moves** (`sink`) unless it is passed read-only; `copy b` makes a deep copy (needs `T` to be copyable).
- It is freed when its owner ends (drop glue: drop the `T`, then free the block).
- It never aliases: there is exactly one box per heap block, so there is no sharing and nothing to race on.
- Under `const` the inside is read-only; under `var` it can be changed (the "const depth" rule, VS5, already in `checker.md`).
- A reference into the inside is a **view** (second-class): it can be passed down to a call but never stored or returned.

This is the whole model; boxes add no new ownership rule. That is deliberate: the less new there is, the less the checker changes.

# 3. Proposed surface (the part the owner decides)

Recommendation (Option A), no new operators:

| Written | Meaning | Mode |
|---|---|---|
| `box(value)` | Make a box holding `value` (moved in). The type name is the call, like `shared(v)` and `lock(v)` (rule T3). | `sink` |
| `b.get()` | A **view** of the `T` inside. It behaves exactly like a field path: `b.get().x` reads, `inout b.get()` can be passed as `inout`, and VS3 applies (taking an owning part out of it needs `copy`). | let / inout |
| `b.set!(v)` | Drop the old value and put `v` there. | `inout b`, `sink v` |
| `b.take!!()` | Consume the box and give back the `T`. | `sink b` |
| `b.field`, `b.method()`, `b[i]` | **Auto-deref**: a box forwards field access, method calls and indexing to the `T`, so recursive code reads naturally. | as the target |
| `copy b` | Deep copy; requires `T: clone`-able (plain data and owning types are; an `obj` needs `clone`). | |

The syntax tree example from `types.md`:

```
enum tree {
    leaf
    node(box<tree>, int, box<tree>)
}

fn insert(t: tree, v: int) -> tree {
    switch t {
        case .leaf: tree.node(box(tree.leaf), v, box(tree.leaf))
        case .node(l, x, r):
            if v < x { tree.node(box(insert(l.get(), v)), x, copy r) }
            else     { tree.node(copy l, x, box(insert(r.get(), v))) }
    }
}
```

Here `l` and `r` are views of the boxes inside `t`, so `l.get()` is a view of a `tree` and passing it down as a read-only argument is allowed.
`copy l` is needed to keep a part of `t` in the result (rule VS3).

Alternative (Option B): add a prefix `*` operator: `*b` reads, `*b = v` writes, `inout *b`. It reads shorter, but it is **new syntax** (the parser has `**` for
power and no unary `*`), a new precedence level, and a new thing to teach. It also invites C habits (`*p` on raw pointers). I recommend A and revisiting B only if `get()` proves noisy
in real code.

# 4. Design details

## 4.1 Checker

1. **Making a box.** The resolver currently treats `box` only as a type name. A call `box(x)` must resolve to a built-in constructor:
   type `fn<T>(sink T) -> box<T>`. Implementation choice: declare `pub struct box<T>` in `core.lang`? No: `box` is already a built-in type kind (`TyKind::Box`) used by
   the layout and glue code. So add a **built-in function entry** (like `ok` / `err`, which are `DefKind::BuiltinFn`): `box`. Inference: the argument's type gives `T`; an expected type `box<U>` (from an annotation or a parameter)
   unifies first, as `check_coerce` does for lists.
2. **Methods.** `get`, `set`, `take` are declared in `core.lang` as `impl<T> box<T> { ... }` with `ext fn` bodies (the compiler supplies them), exactly how `option` and `list`
   methods were first declared. `get` is special: the checker records its result as a *place rooted at the receiver*, not a fresh value. Mechanism: in the value pass (`values/mod.rs`), the
   place analysis already knows `a.b.c` and `a[i]`; add the case "method call named `get` on a `box<T>`" to the same function, producing a path element `Deref`.
3. **Auto-deref.** In `field_expr`, `method_call` and `index_expr` (checker `expr.rs` / `call.rs`): if the receiver type is `box<T>` and lookup on `box<T>` itself fails, retry on `T`
   (repeat for `box<box<T>>`). Record in a side table that the access went through a box so the lowering knows to load the pointer first (the lowering already does this for
   field places: `base_address` has a `Box` arm).
4. **Recursive types.** A struct or enum may name itself only through an indirection: `box<T>`, a list, a map, a set or an `option` of one of those, never directly or through `option<self>`
   (that would be infinite size). Today the layout code breaks cycles only for the drop analysis. Add a **size check** in the checker's declarations pass: compute "directly contains" edges and report
   a cycle as "`tree` contains itself, so it would be infinitely large; put the inner one in a `box`". Without it a bad type crashes the lowering.
5. **Interfaces.** `box<T>` provides `eq`, `order`, `hash`, `show` when `T` does (forwarding), and `clone` when `T` does. `box<T>` is `linear` if `T` is. Add these to `builtin_provides` in `typecheck/iface.rs`.
6. **Print.** `print(b)` prints the inside (as the glue already does). Debug-style wrapping like `box(3)` would be noise.

## 4.2 Value pass

- `box(x)`: consumes `x` (a move), like any `sink` argument.
- A box is an owning type (`is_owning`) already; check that `holds_view` and the escape rules treat a `b.get()` view like a field view.
- `b.take!!()` and `sink b` consume the box; partial moves out of `*b` stay illegal (VS3), which is why `take` exists.
- Exclusivity: `inout b.get()` together with another use of `b` in the same call is the same error as `f(inout s.x, s)`.

## 4.3 Lowering

All of it is small because the representation is a pointer:

| Piece | Work |
|---|---|
| `box(v)` | Allocate `size_of<T>` (at least 1), move `v` in with `into`, result is the pointer (a scalar). Result is a fresh owning scalar: **uses the ledger from A-03** if that has landed; until then it follows the "bound or leaked" rule that objects have today. |
| `b.get()` / auto-deref | Place = the pointer value. Field offsets come from `struct_fields(T)` starting at 0 (no header, unlike `obj`). The existing `Box` branch of `base_address` already does `scalar(base)`. |
| `b.set!(v)` | Build `v` into a temporary, drop the old inside, copy the bytes over. |
| `b.take!!()` | Copy the bytes out to the destination, free the block without dropping the contents, clear the owner's flag. |
| drop glue | `Box(inner)`: if `T` needs drop, call its drop on the block; then `Free`. Check it exists and is not guarded by a cycle break that skips recursive types (`needs_drop` inserts `false` while computing; it must still answer `true` for a type like `tree`). |
| clone glue | Allocate, `clone_into` the inside. |
| `eq`/`cmp`/`hash`/`show` | The `Box` arms exist for `eq` and `show`; add `cmp` and `hash`. |
| `option<box<T>>` | Works as an ordinary enum for now (tag plus pointer). A null-pointer layout trick is a later optimisation (put it on the A-04 list). |

## 4.4 Deep structures: drop and recursion

Dropping a long list built from boxes recurses once per node, which can overflow the stack (the interpreter's host stack too). Two parts to the answer:

- **Now:** accept recursion, and give the interpreter a bigger host stack (run it on a thread with a large stack). Add a test of a 10,000-node chain.
- **Later (note in the card, do not build):** make drop of a self-referential box chain iterative with an explicit worklist, generated only for types that contain themselves.

# 5. Tests (write them first)

Checker tests (`typecheck/tests.rs`, `values/tests.rs`):
1. `box(5)` has type `box<int>`; `const b: box<int> = box(5)` too; `box` with no argument is an error.
2. `b.get()` has type `int`; `b.set!(6)` needs `var b`; `const b` then `b.set!(6)` is the usual "is a `const`" error.
3. A recursive `enum tree` through `box` is accepted; `struct a { x: a }` and `struct a { x: a? }` are rejected with the "infinitely large" message; a mutually recursive pair is rejected; through a list is accepted.
4. Auto-deref: `b.x`, `b.method()`, `b[0]` for a box of struct, struct, list.
5. VS3: `const t = b.get().name` (owning part) needs `copy`; `copy b` fine for `box<string>`; `copy` of a `box<obj without clone>` is an error.
6. Moves: using `b` after `take!!()` is an error; passing `b` to a `sink` parameter moves it.
7. A view of the inside cannot be returned or stored (`fn f(b: box<int>) -> int { b.get() }` returns a copy of an `int`, fine; for a `string` inside it must say `copy`).

Lowering tests (`lower/tests.rs`, each also checks no leaks via `output_of`):
8. Create, read, set, take for `int`, `string`, a struct.
9. The `tree` insert example above, then in-order printing, with 3 and with 1000 values.
10. A singly linked list (`enum list_of { nil, cons(int, box<list_of>) }`), length by recursion, and by a loop with `become`.
11. `copy` of a tree is deep (change the copy, the original keeps its values); both drop without leaks.
12. `box<fn(int) -> int>` returned from a function (rule TC4) and called.
13. `print(box(3))`, `==` and `<` on boxes, `box` as a map key and set element.
14. `option<box<tree>>` as a field.
15. A 10,000-node chain drops without a host stack overflow.

# 6. Steps

1. Owner answers the six questions below. Record answers in `types.md` (new section "box"), `checker.md` (rule list VS16+ if the value pass changes), `ROADMAP.md`.
2. Checker: built-in `box` constructor, `impl<T> box<T>` declarations in `core.lang`, auto-deref, recursion size check, interface provision. Tests 1 to 7.
3. Value pass: `get` place element, `take` consumption. Tests 5 to 7.
4. Lowering and `core.lang` intrinsics (`rt_box_new<T>`, or expand inline). Tests 8 to 15.
5. Docs: `types.md`, `checker.md`, `ir.md` (a new IR decision for the layout: "a box is a bare pointer to the value, no header"), `examples/tour.lang` (one line each for `box(...)`, `get`, `take`), `BLESS=1 cargo test` if the tour changes.
6. Add a real program to `examples/programs/` (`tree.lang`: insert, search, in-order walk) and to the `example_programs_run` test.

# Done means

- All fifteen tests above pass, fmt and clippy are clean.
- `examples/programs/tree.lang` type-checks and runs with no leak.
- `docs/design/types.md`, `checker.md`, `ir.md`, `ROADMAP.md` and `tour.lang` record the decisions.
- The line "Still not lowered: ... `box<T>` values" is gone from `ROADMAP.md` and `ir.md` (IR20 edited).

# Rejected options

- **A reference-counted box by default** (like `Rc`): hides cost and allows aliasing, which breaks "one owner". `shared<T>` already exists for counted read-only sharing.
- **Garbage collection for recursive types:** against the project's decisions (no GC, no runtime).
- **Indexes into a list as the only way** (arena style): possible today, but it makes every tree walk manual and error-prone; boxes are worth having.
- **Raw `ptr` with `unsafe`:** exists for the library, but user code should not need it for a tree.

# Questions for the owner

1. Surface: Option A (`box(v)`, `get()`, `set!()`, `take!!()`, auto-deref, no new operator) or Option B (prefix `*`)? **Recommendation: A.**
2. Should `box(v)` be written as a call to the type name (`box(5)`), or `box.new(5)`? **Recommendation: `box(5)`**, matching `shared(v)` / `lock(v)` / `range(...)`.
3. Auto-deref: forward fields, methods and indexing automatically (**recommended**), or require `.get()` every time?
4. Should `option<box<T>>` get a null-pointer layout (no tag) later? **Recommendation: yes, as an A-04 optimisation; not part of this card.**
5. Deep drop of long chains: recursion plus a bigger stack now (**recommended**), or build the iterative drop in this card?
6. May recursive types be checked for infinite size in the checker now (**recommended**), or left to fail in the lowering?
