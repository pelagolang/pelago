# The checker: names, types and value semantics (draft 3)

How the front end after the parser is built, in stages that each end with something testable.

```
cargo run -- check <file or directory>     # parse + resolve names; later: + types
```

## The stages

| Stage | What it adds | State |
|---|---|---|
| 1. Names | modules, imports, scopes: what every name refers to | **done** (`compiler/src/resolve/`, `package.rs`) |
| 2. Types | a type for every expression, local inference, method lookup, operators | **done** (`compiler/src/types.rs`, `typecheck/`) |
| 3. Generics and interfaces | bounds, `impl` lookup, `any`, the orphan rule | after 2 |
| 4. Patterns | typing of patterns, exhaustiveness of `switch`, list-pattern lengths | after 2 |
| 5. Value semantics | `let` / `inout` / `sink` exclusivity, moves, `copy`, second-class references, `const` depth, `become` | after 3 and 4 |
| 6. Errors and entry points | `?` and `converts`, `main` forms, overflow operators, `defer` | with 5 |

Each stage reports its own errors and leaves a table for the next. The end product is a typed form of the program
that lowering to IR (Phase 2, last bullets) reads.

## Stage 1: names (done)

Input: a `Program` of parsed modules. Output: a table of definitions (`Def`), a map from each use of a name to its
definition, and diagnostics.

- **Files, packages, modules.** A file is a module named after the file; `geometry/shapes.lang` is `geometry.shapes`;
  a made-up empty parent exists when only the directory does. Each package has a root; imports start at your own
  package's top-level modules or at another package.
- **Scopes.** Local scopes (blocks, loops, closures, `switch` arms, builders) over the module scope over the prelude.
  Items of a module are collected first, so order does not matter. Local `fn` and `struct` are hoisted in their block.
- **Imports** are resolved repeatedly until nothing changes, so chains, cycles and `pub import` re-exports work in any
  order. A local definition beats an explicit import beats a `{ ... }` import; two equal imports of one name are an
  error at the use.
- **Binding rules.** A name is bound once per local scope (error otherwise, TC3); a nested `fn` cannot use the
  enclosing function's locals (a closure can); `self` only in methods; `Self` only in types and `impl`s; a module is not
  a value or a type; privacy (`priv`, `pub pkg`, `pub`) across modules; a `mod` line must match the file name.
- **Names bound by `is`** flow into the code the condition guards: `if v is .a(n) && n > 1 { n }`, and after
  `guard v is .a(n) else { return }` the name `n` is in scope for the rest of the block. They do not flow through `||`.
- **Left for stage 2:** everything that needs a type: `p.x`, `p.len()`, leading-dot cases (`.circle`), named-argument
  labels, struct-literal field names.
- **Helpful errors:** "did you mean ...?" from edit distance, and the hints for words from other languages (`null`, `this`).

Tests: about 30 resolution tests, random damaged input, and `langc check` on a directory.

## Stage 2: types (done)

`cargo run -- check file-or-directory` now parses, resolves names and checks types. About 40 tests plus three real programs in
`examples/programs/` (`shapes`, `words`, `stack`) that must check cleanly.

What it does:

- **Types** are interned ids (`Ty`) with inference variables solved by unification and undone with a trail
  (`types.rs`). Number literals are special variables: any integer type (default `int`) or any float type (default `float`),
  and a literal that does not fit its final type is an error (`300` as a `uint8`).
- **Signatures first, bodies second.** Parameters, fields, enum cases and `impl`s are typed without looking at any body.
- **Bidirectional checking.** The expected type flows into `[]`, `none`, `.circle(...)`, number literals, closures and
  `ok(...)` / `err(...)`. Anything still open at the end of a function ("cannot work out the type here") is an error.
- **Expressions:** operators, comparisons, `??`, `?.`, `?`, conversions (`to`, `to?`), ranges, indexing, tuples, lists, sets, maps,
  builders, struct literals (defaults, spreads, shorthand, privacy), calls (positional, named, variadic, defaults, generics),
  closures (`it`, captures, return types), `if` / `switch` / ternary joins, `loop` values, labeled breaks.
- **Methods:** fields and methods of a type and of its `obj` bases, then `impl` blocks. Receiver marks are checked: `!` exactly
  when the method takes `inout self`, `!!` when it takes `sink self`.
- **Patterns:** bindings get types; enum cases (`.circle`, `type.case`, `ok`, `err`, `some`), tuples, lists with `...`, ranges,
  struct patterns (a left-out field needs `..`), `x is dog`.
- **Generics:** type parameters on functions, structs and enums, with inference at calls and `f<int>(...)`. Bounds are not checked yet.
- **The library** is written in the language: `compiler/core/core.lang` is the package `core`, loaded by `Program::with_core()`. Its
  `pub` names are in scope everywhere. Functions that the backend provides are `ext fn` without a body, for example
  `impl<T> list<T> { ext fn push(inout self, sink item: T) }`.
- **`Unknown`** is a quiet type for things not modelled yet (the `scope` handle, `chan`, ...). It unifies with everything so one gap
  does not flood the output.

Not yet (later stages): interface bounds and `impl` completeness, operators for user types, `==` requirements, exhaustiveness of
`switch`, and everything about `inout` exclusivity, moves and `const` depth.

### Choices made while building it (TC9 to TC13)

- **TC9. `sink` at a call site.** A variable passed to a `sink` parameter is written `sink x` (it moves). A temporary needs no mark
  (`xs.push!(2)`). `inout x` must be a variable, never a temporary.
- **TC10. `void ! E` ends with success.** A function returning `void ! E` (and every `init`) may fall off the end, which means `ok`.
  `return err(...)` still exits early. This also makes `fn main() -> void ! error { ... }` natural.
- **TC11. `to`.** Allowed when nothing can be lost: wider integers of the same signedness, unsigned to a wider signed, any integer or
  `float32` to `float`, `char` to a 32-bit or wider integer, `bool` to an integer, anything to `string`, `string` and `str`, an
  `obj` to its base. Float to integer and narrowing do not use `to`.
- **TC12. Methods on number literals.** `1.max(2)` and `xs.first()?.abs()` settle the literal to `int` (or `float`) at the call.
- **TC13. Rules for text.** `"literal"` is a `string`; it converts to a `str` view where one is wanted; `+` joins text; text cannot be
  indexed by position, only sliced by range.

## Decisions from draft 1 (TC1 to TC8)

- **TC1** `5` is an `int` unless the context asks for another integer type (`const b: uint8 = 5` is fine, `300` there is a
  compile error). `2.5` is a `float` unless the context says `float32`. `float` is `float64`, as `int` is `int64`.
- **TC2** Only two implicit conversions: a value to `option` of its type (`int` to `int?`), and an `obj` to its base `obj` or to
  an interface it implements. Everything else needs `to`: no silent widening and no `int` to `float`.
- **TC3** Declaring a name twice in one scope is an error; shadowing in an inner block is allowed.
- **TC4** `fn(int) -> int` is a view type, like a slice: it can be passed down but not stored in a struct or returned, unless it is
  boxed (`box<fn(int) -> int>`). No hidden allocation.
- **TC5** Empty literals (`[]`, `{:}`, `set {}`, `none`) get their type from context or an annotation; otherwise it is an error.
- **TC6** Integer `/` truncates toward zero, `%` takes the sign of the left side, dividing by zero stops the program, and shifting
  by the width or more stops it too.
- **TC7** `==` on floats follows IEEE (`nan != nan`); on lists, strings, tuples and plain structs it compares contents; on `obj`s
  the type must provide `eq`.
- **TC8** Stage 2 (types) comes first in full, with generics and interfaces covered next in stage 3.
