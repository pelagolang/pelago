# Lowering to IR (draft 3)

What happens between "the checker accepted the program" and "the backend turns it into machine code", why it is shaped
this way, and the choices that are yours. Nothing here is built yet.

## 1. Why an IR at all

The checked syntax tree is good for meaning (names, types, errors) and bad for machines: it nests, it has sugar (`?`, `for`,
`switch`, builders, closures, `defer`), and its types are generic. The backend wants the opposite: flat, explicit, and every
type concrete. The **IR** is that middle form. Three jobs, in order:

1. **Lower**: rewrite every convenient construct into a few simple ones.
2. **Specialise** (monomorphise): one copy of each generic function per set of type arguments actually used.
3. **Make ownership visible**: where each value is created, moved and destroyed, so the backend needs no knowledge of the rules.

The checker has already proved the rules (stage 5). Lowering does not re-check them; it **uses** the facts (which variable is
moved on which path) to decide what to destroy and when. A separate **IR verifier** re-checks the IR's own invariants, so bugs in
lowering show up as an IR error and not as a crash in generated code.

## 2. The shape of the IR

Static single assignment (SSA) in basic blocks, the form every optimiser and register allocator expects.

```
fn sum(xs: [int]) -> int {
  b0:
    v0 = const 0
    v1 = len xs
    jump b1(v0, 0)
  b1(acc: int, i: int):                 // block parameters instead of phi nodes
    v2 = lt i, v1
    branch v2, b2, b3(acc)
  b2:
    v3 = index xs, i
    v4 = add_checked acc, v3            // stops on overflow
    v5 = add i, 1
    jump b1(v4, v5)
  b3(result: int):
    return result
}
```

- **A function** is a list of blocks; a **block** is a list of instructions ending in one terminator (`jump`, `branch`, `switch`,
  `return`, `stop`). Values are numbered (`v0`) and assigned once. Block parameters carry values across jumps (the same idea as
  phi, easier to build and to print).
- **Types are concrete and small**: integers of each width, floats, `bool`, pointers, and aggregates (struct, tagged union,
  fixed array). No generics, no interfaces, no inference variables.
- **Select and conditional stops.** `select c, a, b` is `a` when `c` is true, else `b` (both are already computed). `add_checked_if (test), a, b` (also `sub`, `mul`, and unsigned) stops on overflow only when the test holds (a comparison, or a bool and the value it must have) and is the wrapping operation otherwise. The pass `ifconv` makes them: a branch whose two sides only compute a few values (no memory, calls or division) and meet again becomes straight-line code with a `select` per differing value, and a checked operation on a side becomes a `checked_if` for that side, so the program stops where it did before and not for values the other side never saw.
- **Memory** is explicit: `alloc` / `free` / `realloc` for the heap (`realloc` keeps the first bytes and zeroes the rest; a large block moves without copying on Linux), `slot` for stack space, `load` / `store`, `field` / `index` addresses.
- **Index-based storage** (functions, blocks and instructions in flat vectors, referred to by number), as the arena discussion
  already pointed toward. Cheap to build, cheap to free, friendly to caches.
- **A printer** (`pelago ir <file>`) and a **verifier** come first, before any lowering, so each step is testable.

## 3. How each part of the language lowers

| Source | IR |
|---|---|
| `if`, `while`, `loop`, `for` over a range or list | blocks and branches; `for x of xs` is an index loop over the list |
| `switch` and patterns | a decision tree of tag tests and branches; the exhaustiveness check guarantees every leaf exists |
| `?` | test the tag; on failure convert the error (`converts`) and `return`, after running the cleanups of every live scope |
| `??`, `?.` | tag test and branch |
| `defer`, `deinit`, drops | **cleanup code** copied (or shared as a block) at every exit of the scope: normal end, `return`, `?`, `break`, `continue` |
| a moved variable | nothing is destroyed at its scope end; the checker knows it is gone. If it is moved on *some* paths only, a one-bit **drop flag** records it |
| `let` parameter | small plain values by value; everything else by pointer, read-only (no copy) |
| `inout` parameter | a pointer; the checker's exclusivity rule means it can be marked `noalias` for the optimiser |
| `sink` parameter | by value (a move of the bytes); the callee owns and destroys it |
| `copy x` | a call to the type's clone (or a plain copy of bytes for plain types) |
| struct, tuple | an aggregate laid out in declaration order; `@packed` / `@align` later |
| `enum` | a tag plus a union of the cases; `option<T>` and `result<T, E>` use the same scheme (niche tricks, such as a null pointer for `none`, are an optimisation, not a requirement) |
| `obj` | a heap block with a **vtable pointer** first; `open fn` calls load the function from the vtable |
| `any iface` | a pointer to the value plus a vtable of the interface's methods |
| `<T: iface>` generics | **monomorphised**: no vtable, direct calls, fully inlinable |
| closures | a struct holding the captures plus a function taking that struct first; a `fn(...)` value is a (function, environment) pair |
| `string`, `list`, `map` | library structs (pointer, length, capacity) written in the language over a small set of allocator intrinsics |
| `ext fn` | a call to a runtime or system routine the backend provides |
| overflow, bounds, divide by zero | a compare and a branch to a shared `stop` block per function (message and location in a table); `+%` etc. compile to the bare instruction |
| `spawn`, channels, `lock` | calls into the thread runtime; the checker already guaranteed what may cross |

## 4. Specialisation

Starting from `main` (and exported functions), walk the call graph; each call with concrete type arguments asks for the function
"`largest` with `T = int`". Each pair is lowered once and cached. Interfaces with `any` are the only place a call is resolved at
run time. The cost is code size for generics used with many types; the benefit is speed with no indirection and no boxing. The
compile-time cost is paid only for what is reachable, which also removes dead code for free.

## 5. What the first version leaves out

- **No optimisation passes.** The first goal is a correct, readable IR and a verifier. (Constant folding and dead-code removal come
  with the backend; inlining and the no-alias optimisations are Phase 6.)
- **No debug information** beyond a source span per instruction (kept so errors and stop messages can name lines).
- **No unwinding**: a stop aborts the process (E3), so there are no landing pads.
- **Compile-time code** (`meta`) is not run yet; it needs an IR interpreter, which this design makes easy later.
- **Async** is not lowered (still deferred).

## 6. Build order, each step testable

1. IR data types, printer and verifier (`compiler/src/ir/`), with hand-written IR in tests.
2. Lower the simplest functions: integers, locals, `if`, `while`, calls. Print the IR for `examples/programs`.
3. Aggregates: structs, tuples, enums, `option`, `result`, `?`, `switch` decision trees.
4. Ownership: drops, drop flags, `defer`, `sink`/`inout`, `copy`.
5. Monomorphisation and generics; interface calls, `any`, `obj` vtables.
6. Closures, lists, strings, maps, and the intrinsics (`ext fn`) they need.
7. An IR interpreter for tests (run the IR and compare with expected output) before the backend exists.

The milestone of Phase 2 ("programs are type-checked and lowered to IR") is step 5, with step 7 as the proof.

## 7. Decisions (IR1 to IR8, all as recommended)

- **IR1** SSA with block parameters instead of phi nodes.
- **IR2** An IR interpreter is built early (step 7 moves up right after the first lowering), to test lowering by running programs; it later
  becomes the base of `meta`.
- **IR3** Variables moved on only some paths get a one-bit drop flag; the checker keeps allowing them.
- **IR4** `let` parameters of plain types up to 16 bytes are passed by value, larger ones by pointer.
- **IR5** One shared `stop` block per function, with a table of messages and spans.
- **IR6** `list`, `string` and `map` are written in the language over allocator intrinsics (the first self-hosting exercise).
- **IR7** `int` is 64 bits on every target.
- **IR8** The printed IR syntax above (`v0 = add v1, v2`, `b1(acc: int):`) is also the test format.

## 8. Progress

- Step 1 (IR types, printer, verifier) is built: `compiler/src/ir/` (`mod.rs` data and builder, `print.rs`, `verify.rs`). The verifier checks
  one terminator per block, single assignment, definitions that dominate their uses, and types of operands, jumps, calls and returns.
- Step 2 (the first lowering) and the IR interpreter (IR2) are built: `compiler/src/lower/` and `compiler/src/ir/interp.rs`, with
  `pelago ir <path>` to print the IR of a program. It handles functions over integers, floats and `bool`: parameters, `const` and `var`
  (a `var` lives in a stack slot; a later pass can turn slots into values), checked arithmetic, wrapping operators, comparisons,
  `&&` / `||`, `if` / `unless`, `?:`, `while`, `loop` (with `break value`), `for` over a range, labels, `continue`, `return`,
  `to` conversions and calls. Division and remainder stop on zero; shifts stop at the width; `+ - *` stop on overflow.
  Anything else is reported as a note ("not lowered yet: ...") and the function becomes a `stop` so the module still verifies. A
  function whose signature is not simple (lists, strings, `inout`, generics) is left out of the module for now.
- Tests lower source, verify the IR and run it in the interpreter (recursion, loops, labels, narrow and unsigned integers, floats,
  every kind of stop).
- Step 3 (the rest of the language) is built, in `compiler/src/lower/` (monomorphised: one IR function per generic instance).
  `pelago run <path>` lowers a program and runs `main` in the interpreter; the tests run about sixty programs this way, and each one
  also has to finish with no leaked or double-freed memory (the interpreter checks).
  Everything the checker accepts is lowered, with the limits listed below.

### How things are laid out (implementation decisions, IR9 to IR20)

- **IR9** Numbers, `bool`, `char`, `ptr`, `box<T>` and `obj` references are SSA scalars. Everything else lives in memory and is passed by
  address: structs (fields in order, padded), tuples, enums (a tag byte, then the payload of the case), `option` / `result` (enums), fixed arrays
  (inline), `string` (`ptr, len, cap`; a capacity of 0 means "not owned"), `str` (`ptr, len`), lists and `iter` (`ptr, len, cap`; an `iter` is an
  eager list), maps and sets (4 words, below), function values (`code, env`), `any` (`data, table`).
- **IR10** Ownership is run with drop flags: every owned value has a flag byte, set when it is made and cleared when it moves or is
  dropped. Glue functions (drop, copy, equal, order, hash, print) are generated once per type and shared.
- **IR11** The library is written in the language (`compiler/core/core.pel`) over `rt_*` intrinsics (`rt_alloc`, `rt_read<T>`, `rt_size_of<T>`,
  `rt_clone<T>`, `rt_drop_at<T>`, ...): lists, text, `option` / `result` methods, maps, sets, ranges, tasks, channels, `shared`, `lock`. The
  lowering only calls them.
- **IR12** Maps and sets keep insertion order: a block of keys (then values), a length, a capacity and an open-addressing index table of
  positions. Removing shifts the entries down (O(n)) and rebuilds the index. `==` ignores order.
- **IR13** Closures capture by address (views), by value (a copy of an unchanging number) or by `sink` (moved in). The environment starts with
  two words: a drop function (null while it lives on the stack) and a flag that says the body took over the moved-in values. A closure with a
  `sink` capture has its environment on the heap and may leave its function; function values therefore need dropping. A closure that owns data
  cannot be copied and is called once.
- **IR14** An `obj` is a heap block: a table pointer, then the fields (the base's first). The table is static data built on first use: drop, copy,
  the ancestors list (for `is` and `to?`), and the `open` / `override` methods in hierarchy order. Calls to those methods go through the
  table. `to?` moves the object into the `option` when the test passes.
- **IR15** `any I` is a heap copy of the value plus a table for the pair (type, interface): a drop function and one entry per interface method.
  An entry takes the address of the copy where the method wants `self`, and loads it first when the type passes `self` by value.
- **IR16** `init` builds a struct in the return slot (or in a temporary when it can fail); for an `obj` the caller allocates the block and `init`
  gets its address. `deinit` runs before the fields are dropped. Fields with defaults are filled before `init` runs.
- **IR17** Module-level `const` values are evaluated where they are used; a module-level `var` is static data, set up the first time it is reached.
- **IR18** Tasks run one after another (stage 0 has no threads): `spawn` queues the task; it runs when it is joined, when a `recv` finds the
  channel empty, when a `select` has nothing ready, or at the end of `main`. Channels are queues with sender counts, `shared` is a counted
  box, `lock` runs its closure at once. `scope s { s.spawn(...) }` runs each task right where it is spawned. Two tasks that must talk back and
  forth need real threads (a later stage).
- **IR19** `become f(...)` on the function itself with plain number parameters is a jump to the top (no stack growth); any other `become` is a
  call followed by `return`.
- **IR20** Not lowered, on purpose: `meta` (compile-time code), `soa` and `async` (see the plans in `docs/plans/`).
- **IR27** Small closing fixes. `any show` tables for types without a `show` method of their own (numbers, lists, strings...) point their `show`
  slot at the library function `rt_any_show<T>`; this was the real cause of `print(if c { 8 } else { 4 })` failing (the table had no `show`).
  `meta` results can hold `box`, maps and sets (`Cv::Boxed`, `Cv::Entries`; maps and sets are rebuilt with `rt_map_put`/`rt_set_put`).
  The interpreter records the call chain of a failing run (`Outcome.trace`) and meta errors print it. Types carry `+ - *` on compile-time
  numbers (`TyKind::ConstOp`).
- **IR26** `async fn` is lowered as an ordinary function that returns its value; the checker gives callers `future<T>`, and a call site puts a scalar
  result into memory (a `future<T>` has the layout of a `T`). `await x` is parsed as `x.wait!!()`, whose library body is a bit-move out of
  the future. No state machines yet.
- **IR25** `meta expr` runs in the IR interpreter during lowering (`lower/meta.rs`, `run_capture` in the interpreter): the jobs the expression
  needs are lowered first (others wait), the entry function writes its result to a block the interpreter provides, the value is read back
  by type into a tree of constants and emitted as code at the use site. The step limit is a parameter of the run.
- **IR24** `soa<T>` (`lower/soa.rs`): one list header (24 bytes) per field of the struct `T`, one after the other, all the same length. The
  element is never stored whole. `points[i].x` is the address `x_data + i * size` (bounds checked against the length); `points.x` is the
  column header itself, read as a `[float]`; a whole element (`const p = points[i]`) is gathered into a temporary; `points[i] = v` drops the old
  fields and writes the new ones; `for p of points` gathers each element, and `for inout p` scatters the turn's changes back. Drop, clone
  and `==` are those of the tuple of columns; `show` prints a list of structs. `push`, `insert`, `remove`, `clear` loop over the columns
  with the library functions `rt_col_*`. `soa<T, N>` is accepted (N a power of two) and behaves exactly the same; the blocked
  memory layout waits for the backend, where it can be measured. Not supported: `inout points[i]` as a whole-element argument (use a
  field), `soa` of enums, objects or tuples (an error), `...` in a `soa` literal.
- **IR23** **The optimiser** (`compiler/src/ir/opt/`, `pelago run --opt`, `pelago ir --opt --stats`). Per function, repeated until nothing
  changes: simplify the CFG (fold branches, skip empty blocks, merge blocks), dead code, promote slots to values (Braun et al., block
  parameters), split slots at constant offsets (SROA), sparse conditional constant propagation. Then inlining (callees first; small
  functions or single call sites; never recursive ones) and the per-function passes again. Drop flags need no pass of their own: once their
  slot is a value, constant propagation folds the test. Every lowering test runs the program with and without the pipeline and compares
  output, result and leaks. Measured on the sample programs (interpreter steps, before to after): shapes 3658 to 2384, stack 780 to 534,
  tree 7458 to 5462, words 8735 to 5594; slots 31 to 5, 22 to 5, 45 to 16, 96 to 21. `fib` has no slots and no flags. Not yet: whole-slot
  copies between two slots, redundant check removal, common subexpressions, escape analysis (see card A-04, section 7).
- **IR22** **The ledger.** A fresh owning value that is an SSA scalar (an `obj`, a `box`: a call result, an object literal or constructor)
  is *entered* in the ledger when it is made: it is owed a drop at the end of the statement that made it. A consumer that takes it
  (binding, a `sink` argument, a field, `return`, a push, a closure capture, a block's or branch's result) *settles* it. Anything still
  owed at the end of the statement is *swept*: dropped right there. A value that is the result of a block or branch moves out to the
  enclosing scope, and through a join it moves to the block parameter. Memory temporaries (strings, lists, structs) were already owned by
  the statement's scope. Today the entries use a flag slot, as variables do; the optimiser (A-04) turns these into constants.
  Entry and consumers: `fresh_scalar`, `settle`, `pop_scope_carrying` in `lower/func.rs`.
- **IR21** `box<T>` is a bare pointer to a heap block that holds the `T` (no header, unlike `obj`). `box(v)` calls the library function
  `rt_box_new<T>`; `b.get()` is a place (the pointer itself); fields, methods and indexing go through the pointer; `set` and `take` are
  library methods. Environments of closures that are not passed straight to a call live on the heap, so a closure can be boxed or returned.
  The interpreter runs on a thread with a large host stack so that dropping or walking a chain of ten thousand boxes works.

