# The checker: names, types and value semantics (draft 1)

How the front end after the parser is built, in stages that each end with something testable.

```
cargo run -- check <file or directory>     # parse + resolve names; later: + types
```

## The stages

| Stage | What it adds | State |
|---|---|---|
| 1. Names | modules, imports, scopes: what every name refers to | **done** (`compiler/src/resolve/`, `package.rs`) |
| 2. Types | a type for every expression, local inference, method lookup, operators | next |
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

## Stage 2: types (next)

Design in outline, so you can object early:

- **Types are interned small ids**; equal types have equal ids, so comparing is a number compare. Type variables for
  inference live in a union-find table.
- **Signatures first, bodies second.** First every function, field, variant and `impl` gets its signature (needs no
  bodies). Then bodies are checked, one function at a time; that is also the unit of parallelism later.
- **Local inference only.** Function parameters and return types are always written, so inference never crosses a function
  boundary. Inside a body, `const x = 5` infers; closure parameters are inferred from where the closure is passed.
- **Bidirectional checking**: expressions are checked against an expected type when there is one (so `[]`, `{:}`, `none`,
  `.circle(...)`, integer literals and closures get their meaning from context) and synthesise one otherwise.
- **Operators** go through interfaces (`add`, `eq`, `order`, ...) with built-in implementations for numbers, so user types
  can have them and generic code can ask for them.
- **Method lookup**: fields and inherent methods of the type, then methods from interfaces it implements, then `obj`
  base types. More than one candidate is an error; write the interface name to choose.
- **Output**: a table from each expression to its type (keyed by file and position until tree nodes have ids), plus
  resolved method and field targets.

## Questions the checker needs answered (TC1 to TC8)

**TC1. Numeric literals.** `5` is an `int` unless the context asks for another integer type (`const b: uint8 = 5` is fine, and
`300` there is a compile error); `2.5` is a `float` unless the context says `float32`. Is `float` the same as `float64`
(as `int` is always 64 bits)? I suggest yes.

**TC2. Implicit conversions.** Only two: a value to `option` of its type (`int` to `int?`, decided earlier) and an `obj`
to its base `obj` or to an interface it implements (`dog` to `animal`). Everything else needs `to` (no silent widening
from `int32` to `int`, no `int` to `float`). Fine?

**TC3. Same-scope redeclaration.** `const a = 1` then `const a = 2` in one block is an error today (shadowing in an
inner block is allowed). Rust allows both; TypeScript allows neither. Keep the error?

**TC4. Closure types.** A closure that captures variables cannot be a plain function pointer. I suggest `fn(int) -> int`
is a *view* type, like a slice: it can be passed down (to `map`, `each`, `spawn` with `sink`), but not stored in a
struct or returned, unless it is boxed: `box<fn(int) -> int>`. That fits second-class references and needs no hidden
allocation. Alternative: every `fn` type is a pointer plus an environment pointer and closures may be stored freely
(costs an allocation when it captures). Which?

**TC5. Empty literals.** `[]`, `{:}`, `set {}` and `none` need a type from context or an annotation; `const xs = []` alone
is an error ("what is this a list of?"). Fine?

**TC6. Division and overflow.** Integer `/` truncates toward zero, `%` has the sign of the left side (like C, Rust, Java), and
dividing by zero stops the program like overflow does. Shifts by the width or more also stop. OK?

**TC7. Equality.** `==` on floats follows IEEE (so `nan != nan`); `==` on lists, strings, tuples and plain structs compares
contents; `==` on `obj`s needs the type to provide `eq`. Fine?

**TC8. Order of the work.** Stage 2 (types) in full before stage 3 and 4, with generics and interfaces limited at first to
the built-in ones (`option`, `result`, lists, maps)? That gets small real programs checked soonest. Or generics in the
same pass as stage 2?
