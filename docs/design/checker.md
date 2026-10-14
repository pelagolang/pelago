# The checker: names, types and value semantics (draft 4)

How the front end after the parser is built, in stages that each end with something testable.

```
cargo run -- check <file or directory>     # parse + resolve names; later: + types
```

## The stages

| Stage | What it adds | State |
|---|---|---|
| 1. Names | modules, imports, scopes: what every name refers to | **done** (`compiler/src/resolve/`, `package.rs`) |
| 2. Types | a type for every expression, local inference, method lookup, operators | **done** (`compiler/src/types.rs`, `typecheck/`) |
| 3. Generics and interfaces | bounds, `impl` lookup, `any`, operators, the orphan rule | **done** (`typecheck/iface.rs`) |
| 4. Patterns | exhaustiveness of `switch`, irrefutable patterns, list-pattern lengths | **done** (`typecheck/exhaust.rs`) |
| 5. Value semantics | `inout` exclusivity, moves, `copy`, `const` depth, closure captures | **done** (`values/`), including `become`, `linear`, view escape, `spawn` captures |
| 6. Errors and entry points | `?` and `converts`, `main` forms, overflow operators, `defer` | **done** |

Each stage reports its own errors and leaves a table for the next. The end product is a typed form of the program
that lowering to IR (Phase 2, last bullets) reads.

## Stage 1: names (done)

Input: a `Program` of parsed modules. Output: a table of definitions (`Def`), a map from each use of a name to its
definition, and diagnostics.

- **Files, packages, modules.** A file is a module named after the file; `geometry/shapes.pel` is `geometry.shapes`;
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

Tests: about 30 resolution tests, random damaged input, and `pelago check` on a directory.

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
- **Generics:** type parameters on functions, structs and enums, with inference at calls and `f<int>(...)`. Bounds are checked in stage 3.
- **The library** is written in the language: `compiler/core/core.pel` is the package `core`, loaded by `Program::with_core()`. Its
  `pub` names are in scope everywhere. Functions that the backend provides are `ext fn` without a body, for example
  `impl<T> list<T> { ext fn push(inout self, sink item: T) }`.
- **`Unknown`** is a quiet type for things not modelled yet (the `scope` handle, `chan`, ...). It unifies with everything so one gap
  does not flood the output.

### Decisions TC9 to TC13 (confirmed by the owner)

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

## Stage 3: interfaces (done)

Where it lives: `typecheck/iface.rs`. The library interfaces are in `core.pel`: `show`, `eq`, `order : eq`, `hash : eq`, `clone`,
and one per operator (`add`, `sub`, `mul`, `div`, `rem`, `neg`).

- **What a type provides.** An interface is provided when the type lists it after `:`, has an `impl iface for type`, or has
  `@derive(iface)`; or it is a built-in type the library gives it to (`int` has `eq order hash show clone add ...`, a list has
  `eq order hash show clone` when its elements do); or it is a plain `struct` / `enum` whose fields provide `eq` or `clone` (T5: those
  two are automatic; `order`, `hash`, `show` are opt-in; an `obj` must write all of them). A type parameter provides its bounds, and an
  `any iface` provides what `iface` builds on. An `impl` of `order` does not by itself give `eq`; the type must provide it too.
- **Bounds** (`<T: order + hash>`, `where T: eq`) are queued when a generic is instantiated (a call, a struct literal, a written type
  `bag<k>`) and checked at the end of the function, when the types are known: "`k` does not provide `hash`, which the type parameter `T`
  needs; write `@derive(hash)` on `k` or `impl hash for k`". Library methods carry bounds: `contains` needs `eq`, `sort` needs `order`.
- **Methods through interfaces.** Inside `<T: shape>`, `x.area()` finds the promised method; default methods (an interface `fn` with a
  body) are found on every type that provides the interface; `any shape` calls work the same way. Inside an interface, `Self` stands for
  the type that provides it.
- **`impl` checks.** Every promised method must exist with the same parameters, modes and result (`Self` replaced by the type); the
  interfaces an interface builds on must be provided too; the thing after `impl ... for` must be an interface. Types that list interfaces
  after `:` get the same check.
- **Operators on your own types.** `a + b` looks up `add` on the type and uses its signature (the right side can be another type);
  `-a` uses `neg`; `a += b` is `a = a + b`. `==` needs `eq` and `<` needs `order` on `struct`, `enum`, `obj`, type parameters and
  containers of them; comparing with `none` needs nothing. `copy x` needs `clone`.
- **`any` coercion.** A value goes into `any iface` only if its type provides `iface` (`print(x)` needs `show`).
- **Orphan rule (IM-orphan, now enforced).** An `impl` is allowed when its interface or its type was defined in the same package. Only
  the `core` package may add methods to built-in types.

## Stage 4: exhaustiveness (done)

Where it lives: `typecheck/exhaust.rs`. Patterns are put into a small normal form and checked with the classic matrix method: take the first
column, try each constructor of its type (an enum case, `some` / `none`, `ok` / `err`, `true` / `false`, the one constructor of a tuple or
struct, each list length up to one past the longest pattern), keep the rows that can match, recurse.

- A `switch` must cover every value or have a `default` (or a catch-all name). The error names a value that is missed:
  "this `switch` does not handle `.rect(_, _)`".
- An arm with a guard may fail, so it does not count. Alternatives (`case a, b`) count separately.
- Numbers, text and characters can only be covered by a name or `_`; ranges and `is dog` tests are never complete on their own.
- Patterns that must always match (`const (a, b) = pair`, `var .circle(r) = s`, `for (k, v) of m`, `[a, b] = xs`) get the same check:
  "this pattern does not always match (it misses `[]`); use `switch`, `if ... is` or `guard`".
- Rows that cannot be reached are not reported yet.

## Stage 5: value semantics (done, v1)

Where it lives: `values/` (its own pass after the types, reading `Typed`). It follows `docs/design/types.md` section 2 and `memory.md`.

- **VS1. Permission to change.** A place may be assigned to, passed as `inout`, or used as the receiver of `c.bump!()` only when its root
  variable allows it: a `var`, an `inout` or `sink` parameter, `inout self`, `init` parameters, `inout x` in a pattern. A `const`, a
  read-only parameter (the default), a method's plain `self` and a name bound by a pattern are fixed all the way down (`const` depth).
- **VS2. Moves.** A value that owns something (see `Typed::is_owning`: `string`, lists, maps, sets, `box`, an `obj`, a struct or enum with
  such a field) moves when it is assigned, put into a literal or an enum case, passed to a `sink` parameter, returned, or captured with
  `sink`. Afterwards the variable cannot be used until it is assigned again: "`a` was moved on line 3 and cannot be used again; write
  `copy a` there to keep it". Plain values (numbers, `bool`, plain structs) copy. A type parameter counts as plain for now, because
  generic code cannot say yet whether `T` copies (this tightens when `linear` and copy bounds arrive, M4).
- **VS3. Taking a part needs `copy`.** `return self.name` or `const n = p.name` for an owning field is an error; write `copy self.name`.
  Taking a read-only or `inout` parameter whole is the same error (`fn f(s: string) -> string = s` needs `copy s` or `sink s`). This is
  T1 applied strictly. `init` parameters are owned, so `self.name = name` is fine.
- **VS4. Branches and loops.** After an `if` or `switch` a variable is moved when any side moved it. A loop body is walked twice, so moving
  a variable declared outside the loop shows up as a use after a move. Code after `return`, `break`, `continue` and calls that never
  return is not checked.
- **VS5. Exclusive access.** In one call, a place that is changed or given away (`inout x`, `sink x`, `c.bump!()`, `c.close!!()`) may not
  overlap another place passed to the same call (`both(inout xs, xs)`, `xs.extend!(xs)`, `swap(inout a[0], inout a[1])`). Different fields
  do not overlap; any two indexes of one list do. Temporaries (`xs.push!(xs.len())`) are evaluated first and do not count; plain values
  that are copied do not count. Use `xs.swap!(0, 1)` for two elements of one list.
- **VS6. Closures.** A closure changes an outer variable only if it captures it with `inout` (`[inout total] x ~> total += x`); `sink`
  moves it in at the closure.
- **VS7. `become`.** The call replaces this function's frame, so nothing local may be lent to it: a local owning value is passed with
  `sink x` or `copy x`, a local may not be passed `inout`, and `become` cannot follow a `defer`. Parameters that came from the caller
  (read-only or `inout`) can be passed on.
- **VS8. Views do not outlive their source.** `str` and function values are views: a struct field or enum case cannot hold one (use
  `string`, or `box<fn ...>`), a function cannot return a function value, and a function returning `str` cannot return a view into
  one of its own owning locals (`const s = a + b; s.trim()` is an error; `s to string` is not).
- **VS9. `spawn`.** A closure given to `spawn` must move in what it uses: an owning outer variable needs `[sink x]` (or `copy x`
  first), plain values are copied, and `[inout x]` is an error (use `lock`). `spawn`, `chan`, `task`, `shared`, `lock` are in the
  library (`core.pel`); `for x of rx` loops over a receiver; more senders come from `copy tx`.
- **VS10. `linear` types.** A `linear` struct, obj or enum must be consumed exactly once: on every path out of its scope (end of
  block, `return`, `?`, `break`, `continue`) a variable holding one must have been moved (a `sink` call, a `!!` call, `_ = x`,
  destructuring); dropping it by assignment or by ignoring a call's result is an error. It cannot have `deinit`, and a non-linear type
  cannot hold one. `sink` parameters of a linear type count as variables.
- **VS13. `T: linear`.** A type parameter may hold a linear value only if it says so (`<T: linear>` or `where T: linear`); instantiating a
  plain `T` with a linear type is an error ("`txn` is `linear`, so it cannot be used for `T`; declare `T: linear`"). Inside such a
  function a `T` value is tracked like any linear value. The library's own generics (`list<T>`, `option<T>`, ...) accept linear
  values, and a container of linear values is linear. `linear` is not an interface, so the resolver skips it as a bound.
- **VS14. Linear values and closures.** A closure consumes an outer linear variable only if it captures it with `[sink x]`, and then it
  must consume it before the closure ends.
- **VS16. Boxes.** `b.get()` is a place inside the box (a view, like a field): taking an owning part out of it needs `copy` (VS3). `b.take!!()` is the only way to move the value out. Fields, methods and indexing go through a box. A type that contains itself without a pointer is rejected ("infinitely large").
- **VS15. Warnings.** Diagnostics now have a severity. An unreachable `switch` arm is a warning; `pelago check` prints it and does not
  fail on it. Everything else stays an error. (Tests count warnings as unexpected output, so they stay at zero.)
- **`async`** stays deferred (G5): the words are reserved, and nothing in the checker needs it.
- **VS11. `defer`.** Code in `defer` cannot `return`, `?`, `break`, `continue` or `become`, and its result cannot be a `result`
  (nowhere to send the error). Names it uses are checked where the `defer` is written; what it moves happens at scope end.
- **VS12. Plain values need no `sink`.** The `sink` mark at a call is required only for values that own something; `tx.send(i)` with an
  `int` is a copy. (Refines TC9.)

## Stage 6: errors and entry points (done)

- **`converts`.** `?` and `to` use `impl converts<app_error> for parse_error { fn convert(self) -> app_error = ... }`. `?` accepts an
  error type when it equals the function's, when the function returns the catch-all `error`, or when such an `impl` exists. The orphan
  rule counts the interface's type argument as local (`app_error` is yours). `converts` is still a placeholder name.
- **`main`.** `fn main()` or `fn main(args: [string])`, returning nothing or `void ! E`; not generic.
- **Overflow.** `+%`, `-%`, `*%` and their assignment forms are for integers (checked in stage 2); plain `+ - *` stop the program on
  overflow at run time.
- **`defer`** rules are VS11 above.
- **Unreachable `switch` arms** are warnings (VS15): "this case can never match: the cases above already cover it".

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
