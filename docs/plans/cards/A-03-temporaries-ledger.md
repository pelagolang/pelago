Id: A-03
Title: The ledger: every fresh value is settled or swept (no leaked temporaries)
Column: Done (scalar owning temporaries; flags remain until A-04)
Order: 3
Depends on: nothing (A-02 will use it)
Size: L
Touches: lowering (`func.rs`, `expr.rs`, `call.rs`, `agg.rs`, `pat.rs`, `seq.rs`, `closure.rs`, `obj.rs`), tests

# 1. The problem, exactly

Values that own something (a string, a list, a map, an `obj`, a `box`, a closure that owns data, an `any`) must be dropped exactly once.
The lowering handles three kinds of owner today:

1. **Named variables and parameters**: a drop item plus a **runtime flag** byte, cleared on a move.
2. **Memory temporaries** (strings, lists, structs...): `rvalue_expr` makes a slot and calls `own(slot, ty)`, which registers a drop item with a runtime flag at the **end of the enclosing scope** (a block), not at the end of the statement.
3. **Scalar owning values** (`obj`, `box`): these are SSA values, not memory. They are owned only if they are stored in a variable; an expression result that nobody binds is simply forgotten.

Case 3 leaks. Examples that leak today:

```
make_dog("rex")                    // an expression statement whose result is an obj: never dropped
print(dog { name: "a" }.name)      // the temporary object behind the field read
f(dog { name: "a" })               // passed by `let`: the caller made it, nobody frees it
const x = if c { dog {...} } else { dog {...} }.name
```

Case 2 does not leak but is **wasteful**: a big temporary lives until the end of the block, and every one pays a flag store and a flag test at run time.

# 2. Why not just copy a known model

- **Rust-style "owner + borrow checker"**: needs lifetimes. We have none: references are second-class views (they cannot be stored or returned), so there is nothing to track beyond one expression.
- **Reference counting everywhere**: runtime cost on every copy; the project goal is no hidden cost.
- **C++-style temporaries destroyed at the end of the full expression**: the right *idea* (and what we want), but C++ does it with rules baked into the language and unwinding.
- **A runtime flag for everything** (what we do for named variables): correct but slow, and it blocks the optimiser from removing the flags (A-04 has to prove them constant).

Our situation has a property none of those had: the checker has already proved every program valid. Moves, exclusivity and view rules are settled before lowering. So the lowering never has to *discover* ownership; it only has to **keep the books for fresh values while it builds code**. That is a compile-time ledger, and it needs **no run-time flag** in the straight-line case.

# 3. The model: the ledger (settle or sweep)

Every lowering function keeps a **ledger**: a stack of lists, one list per *full expression* (a statement, a condition, a loop header, a `switch` arm).

- **Enter.** When an expression produces a **fresh owning value** (a new object, a function result of owning type, a string just built, a box just made...) the lowering *enters* it in the current ledger list: `(where it lives, its type)`. "Fresh" is a property the lowering already knows at each producer; a view of an existing place is **not** fresh.
- **Settle.** When a consumer **takes ownership**, it *settles* the entry: the entry is removed and no code is emitted. Consumers: binding to a `const`/`var`, storing into a field or element, passing to a `sink` parameter, `return`, closure `sink` capture, `rt_write`, `spawn`, assigning (`store_new`), pushing into a collection.
- **Sweep.** When the full expression ends, every entry that is still in its list is **swept**: a drop is emitted for it, right there. The value that nobody took is destroyed at the earliest correct moment.
- **Early exit.** `return`, `break`, `continue` and `?` sweep every ledger list between here and the target, exactly as they already run scope drops, then leave.
- **Branches.** A `then` branch, an `else` branch, a `switch` arm, a loop body and the right side of `&&` / `||` / `??` each open their **own** list (a nested sub-ledger). Values entered inside a branch are settled or swept inside that branch, so they never leak across a join. A value that is the *result* of a branch is settled by the branch into the join's destination.

Properties:

- **No flag in straight-line code.** Whether an entry is settled is known when the lowering is generated. A swept entry is dropped unconditionally; a settled one costs nothing.
- **A flag only when ownership is truly dynamic.** That means: a named variable moved on only one path (already has a flag), or a temporary entered before a conditional consumer. In the second case the ledger entry owns a flag that the conditional consumer clears. Both are rare, and A-04 removes many of them.
- **Earlier drops.** Temporaries die at the end of their statement, not their block. Peak memory goes down and cache behaviour improves.
- **Order is the reverse of entry**, as drops always have been.

Name of the model in docs and code: **the ledger**. Entries are "owed values". `settle` and `sweep` are the two verbs. This vocabulary is ours; do not import "owner", "borrow" or "lifetime" into it.

# 4. What changes in the code

Everything lives in `compiler/src/lower/`.

1. **Data** (`func.rs`): `FnState.ledger: Vec<Vec<Owed>>` where `Owed { ptr_or_value, ty, flag: Option<Value> }`. API:
   - `ledger_open()` / `ledger_close()` around full expressions and branches (close = sweep the list, then pop).
   - `fresh(v, ty)`: enter. For memory values `v` is the slot address; for scalars it is the SSA value, spilled to a small slot only when a drop is actually emitted (spilling at sweep time, not at enter time).
   - `settle(v)`: remove the entry for `v` (by identity). Panics in debug builds if the value is not in the ledger (this catches wrong consumer code early).
   - `sweep_to(depth)`: emit drops for lists above `depth` without popping (used by early exits, like `exit_scopes`).
2. **Producers** call `fresh`: `call_with` results (owning `Sret` or scalar results), `obj_literal_value`, `obj_construct`, `closure_value` when it owns, `coerce_to_any`, `rvalue_expr`'s temp slot, `string_lit`, `concat`, `list_lit` (when used as a temp), `convert_mem`, `opt_call`, `builder_expr`, `show` temporaries, `rt_read`/`rt_clone` results.
   **The existing `own()` for temporaries is replaced by `fresh()`**; `own()` stays for named variables only (they need the flag).
3. **Consumers** call `settle`: `decl` (binding), `fill_struct` / `tuple_lit` / `enum` constructors (values moved into fields), `lower_arg` for `sink`, `return_expr`, `store_new`, `move_local_into`, `closure_value` sink captures, `rt_write`, `rt_cast`, `with_dst`/`into` when the destination takes ownership (this is the common one: **`into(e, dst, ty)` always settles what it builds**, because `dst` becomes the owner).
4. **Statement boundaries**: `stmt` (expression statements), `decl` (after binding), `cond_branch` (condition), `while` / `for` header, `if` arms, `switch` arms, `guard`.
5. **Remove `own(slot, ...)` calls** that registered temporaries (about 25 call sites; grep `self.own(`). Keep them where the thing is a variable.
6. **Verify with the existing leak counter**: the interpreter already reports a leak or a double free; the whole suite must stay clean.

# 5. Order of work (small, safe steps)

1. Add the ledger structure and the verbs, unused. Tests still pass.
2. Use it for **scalar owning values only** (objects, boxes, closures, `any` data): this fixes the real leaks. Add the leak tests below. Nothing else changes.
3. Switch **memory temporaries** to the ledger one producer at a time, running the full suite after each (they currently work, so any failure is a bug in the new code).
4. Delete the old temporary registration and the flag stores that are no longer needed. Report the **count of flag stores in the IR of the sample programs before and after** in the card (a metric for A-04).
5. Write the model into `docs/design/ir.md` (new decision IR21: "the ledger") and into the "How things are laid out" list.

# 6. Tests

Each test runs a tiny program through `output_of`, which already fails on a leak, a double free or a use after free.

1. Unused object result: `make_dog("a")` as a statement.
2. Temporary object field read: `print(make_dog("a").name)`.
3. Object passed to a `let` parameter: `f(dog { name: "a" })`.
4. Object result of an `if` used for a field: `(if c { dog_a() } else { dog_b() }).name`.
5. Temporary string in an argument: `f("a" + "b")`; in a condition; in a `while` condition run 3 times.
6. Result of `?` operand dropped on the error path.
7. Early `return` inside a statement that holds a temporary: `return g(make_dog("a"))`.
8. `break` and `continue` out of a loop body that holds a temporary.
9. A temporary created on only one side of `&&` or `??`.
10. Closure with `sink` capture created and never called; called once; passed to a `let` parameter.
11. `any` created and discarded.
12. Settle checks: moving a temporary into a struct field, a list push, a map insert, a `sink` argument, a return value, each with no leak and no double drop.
13. **Order test**: temporaries with a `deinit` that prints; assert the printed order equals "reverse of creation, at the end of the statement".
14. A regression test that counts allocations: a loop of 1000 iterations that makes a temporary string each time must have a peak of 1 live temporary (add a `peak_live` counter to the interpreter's `Outcome`).

# 7. Risks

- **A consumer forgets to settle** → double drop. The debug assertion in `settle` and the interpreter's double-free detector catch it in tests.
- **A producer forgets to enter** → leak. The leak counter catches it.
- **Conditional consumers** (a temporary entered before a branch that may or may not consume it): handle by giving that entry a flag, as today. Keep the old code path as a fallback inside `fresh_flagged`. Expect two or three such cases (`??` with owned left side, `switch` on a temporary scrutinee).
- **Scrutinee temporaries** (`switch f() { case ... }`): the value lives for the whole `switch`, so the ledger list for a `switch` covers the arms, not just the scrutinee expression.

# Done means

- All 14 tests pass; the full suite is green; fmt and clippy clean.
- `grep -n "self.own(" compiler/src/lower` shows only named variables and parameters.
- The sample programs' IR has measurably fewer flag stores (numbers recorded in the card).
- IR21 written in `docs/design/ir.md`; `docs/design/memory.md` gets a short paragraph describing the ledger in user terms ("a temporary is destroyed at the end of its statement").
- The "values that are never bound can leak" limit is removed from the notes (`ROADMAP.md`, `ir.md`, `S-03`).
