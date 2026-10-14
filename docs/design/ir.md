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
- **Memory** is explicit: `alloc` / `free` for the heap, `slot` for stack space, `load` / `store`, `field` / `index` addresses.
- **Index-based storage** (functions, blocks and instructions in flat vectors, referred to by number), as the arena discussion
  already pointed toward. Cheap to build, cheap to free, friendly to caches.
- **A printer** (`langc ir <file>`) and a **verifier** come first, before any lowering, so each step is testable.

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
  `langc ir <path>` to print the IR of a program. It handles functions over integers, floats and `bool`: parameters, `const` and `var`
  (a `var` lives in a stack slot; a later pass can turn slots into values), checked arithmetic, wrapping operators, comparisons,
  `&&` / `||`, `if` / `unless`, `?:`, `while`, `loop` (with `break value`), `for` over a range, labels, `continue`, `return`,
  `to` conversions and calls. Division and remainder stop on zero; shifts stop at the width; `+ - *` stop on overflow.
  Anything else is reported as a note ("not lowered yet: ...") and the function becomes a `stop` so the module still verifies. A
  function whose signature is not simple (lists, strings, `inout`, generics) is left out of the module for now.
- Tests lower source, verify the IR and run it in the interpreter (recursion, loops, labels, narrow and unsigned integers, floats,
  every kind of stop).
- Next, step 3: aggregates (structs, tuples, enums, `option`, `result`, `?`) and `switch` as decision trees.
