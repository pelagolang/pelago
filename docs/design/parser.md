# Parser (draft 2)

The stage 0 parser is in `compiler/src/parser/` and the tree in `compiler/src/ast.rs`. It parses every
construct decided in the other design docs, and everything in `examples/tour.lang`. This page lists
the grammar rules the parser needed that the other docs did not spell out, so you can confirm or change them.

```
cargo run -- parse examples/tour.lang       # print the tree and any errors
cargo run -- parse-bench big.lang           # lex + parse speed
BLESS=1 cargo test                          # rewrite compiler/tests/golden/*.ast after an intended change
```

## How it works

- **Recursive descent** for files, items, statements, types and patterns; **precedence climbing** for operators.
- **Never fails.** Errors are collected, the tree gets `Error` nodes, and parsing resumes at the next statement
  or declaration. Nesting is limited to 96 levels so hostile input cannot overflow the stack.
- **The tree is plain `Box` / `Vec`** and names are spans of the source (no copying). An arena and interned names
  are a later speed-up that touches only `ast.rs` and the parser.
- Tests: about 45 test functions covering several hundred snippets, a golden tree for each example, and random-input tests (token soup, damaged
  and truncated copies of `tour.lang`) that must not panic or hang.

## Operator order (loosest first)

| Level | Operators |
|---|---|
| assignment (right) | `=` `+=` `-=` `*=` `/=` `%=` `&=` `\|=` `^=` `<<=` `>>=` `+%=` `-%=` `*%=` `??=` |
| ternary (right) | `a ? b : c` |
| 1 | `\|>` |
| 2 | `??` |
| 3 | `\|\|` |
| 4 | `&&` |
| 5 | `==` `!=` `<` `>` `<=` `>=`, `is`, `in` |
| 6 | `..` `..=` and `step` |
| 7 | `to` `to?` |
| 8 to 11 | `\|` then `^` then `&` then `<< >>` |
| 12 | `+` `-` `+%` `-%` |
| 13 | `*` `/` `%` `*%` |
| tighter | prefix `-` `!` `~` `copy`, then `**` (right, so `-2 ** 2` is `-(2 ** 2)`) |

Then postfix: `.field`, `?.field`, `t.0`, `f(args)`, `c.bump!()`, `c.close!!()`, `xs[i]`, `f()?`, `max<int>`,
`point { ... }`.

## Spacing rules the parser relies on

| Rule | Examples |
|---|---|
| A `?` touching the token before it is error propagation, otherwise it is a ternary | `f()?` and `a ? b : c` |
| A `<` touching a name opens generic arguments, otherwise it is less-than | `map<string, int>` and `a < b` |
| A `[` touching an expression is indexing, otherwise it starts a list | `xs[0]` and `return [0]` |
| A `!` or `!!` touching a name and followed by `(` marks the call | `c.bump!()` and `a != b` |
| A type's `?` must touch the type | `int?` is `option<int>`; `x to int ? a : b` is a ternary |

## Newlines

A newline ends a statement, except:

- inside `( )`, `[ ]` and the braces of a struct, map or set literal;
- after a binary operator, `=`, `,` or `->`;
- when the next line starts with `.`, `?.`, `\|>`, `&&`, `\|\|`, `??`, or a ternary `?` (so method chains,
  pipelines and `? :` can be written one step per line);
- before an `else` that starts the next line.

A block's last expression, when not followed by `;`, is its value. Arms of a `switch` end at the next
`case` / `default` / `}`, so `switch x { case 1: "a" case 2: "b" }` works on one line.

## Decisions from draft 1

- **P1** A file may contain statements at the top level, as well as declarations.
- **P2** `use` stays as another spelling of `import`. The import form is `import std.io`, `import fmt = std.io`
  and `import { a, b: c, ... } = std.io` (see `modules.md`); relative paths start with dots.
- **P3** Struct literals are not allowed in conditions; wrap them in parentheses.
- **P4** Bitwise operators bind tighter than comparison (Rust and Go, not C).
- **P5** `to` binds between `+` and the comparisons: `a + b to float` is `(a + b) to float`.
- **P6** Ranges bind tighter than comparison and looser than `to` and `+`; `step` follows a range.
- **P7** `loop` is an expression: `break value` leaves it with a value (`const r = loop { break 5 }`).
- **P8** Loop labels exist (below).
- **P9** Closures may name their return type (below).
- **P10** `T??` is not written; use `(T?)?`.

## Loop labels

```
outer: for i of 0..10 {
    inner: for j of 0..10 {
        if i * j > 20 { break outer }        // leaves both loops
        if j > i { continue outer }          // next i
    }
}

search: loop { break search 5 }              // leave with a value
found: { if ok { break found 1 }  2 }        // a labeled block can be left early too
```

- A label is `name:` in front of a `for`, `while`, `loop`, `do` or a block, at the start of a statement.
- `break name`, `break name value` and `continue name` use it. A plain `break` still leaves the innermost loop.
- A name after `break` is read as a label when an enclosing loop carries that label; otherwise it is a value. To break
  with a value that happens to share a label's name, write it in parentheses: `break (outer)`.
- Labels do not reach into closures or functions. The same label cannot be nested inside itself.
- Not allowed: labels in the middle of an expression (`const r = a: loop { }`), since `a:` already means a map key,
  a named argument and a field there.

## Closure return types

```
const twice = (x: int) -> int ~> x * 2
const bump = [inout n] (a: int, b: int) -> int ~> { n += 1; a + b }
```

Why now: closures that return `result` or `option` often need the type spelled out because inference has nothing
to go on (`~> ok(1)` could be `int ! e` for any `e`), and a block body with several `return`s has the same problem.
The `-> Type` goes after the parameter list, as in a function. It needs the parenthesised parameter form; a lone
`x ~> ...` has no room for it, so use `(x) -> T ~> ...`.

## Doc comments

`///` lines and `///* ... *///` blocks that sit right before a declaration are kept in the tree (`docs` on items,
fields and enum cases) and `doc_text` turns them into plain text. A doc comment on the same line as code before it
belongs to nothing. Doc comments never reach the compiled program; they exist only for tools. A plain compile
skips them: `ParseOptions::compile()` (or `langc parse --no-docs`) does not even collect them. Comments elsewhere are still dropped; the formatter will keep ordinary comments later.

## Syntax checks (`check.rs`)

The parser accepts a little more than the language allows. `parse_and_check` (used by `langc parse`) also runs
these rules, built on the tree walker in `visit.rs`:

| Area | Rule |
|---|---|
| Jumps | `break` and `continue` only inside a loop (a closure starts afresh); `become` only with a call |
| Assignment | the target is a name, field, index or tuple position (`_ = f()` is allowed); `++` / `--` only on those |
| Methods | `self` only as the first parameter of a method; `init` and `deinit` only inside a type; `init` has no `self`; `deinit` takes nothing and returns nothing |
| Parameters | each has a type; no repeated names; a variadic one comes last and alone |
| Bodies | a function has a body, except in interfaces, `abstract` methods and `ext` functions, which have none |
| Declarations | `const` / `var` get a value (not `ext`, which gets none), carry no `inout` / `sink` / `let`, and use patterns that cannot fail by shape |
| Modifiers | `open` / `abstract` / `final` on `obj` and its methods, `override` on methods of an `obj`, `linear` on types, `static` / `ext` on functions and variables, `pub` / `priv` not on `impl` or `test` |
| Types | `interface` and `impl` hold functions only |
| Patterns | a name is bound once per pattern, and the alternatives of one `case` bind the same names in the same modes |
| `switch` / `select` | at most one `default`, and it comes last |
| Builders | a map builder gives `key : value`; a builder starts with `x of xs` |
| Names | no field repeated in a struct literal, no argument name repeated in a call |

Rules that need types (are alternatives of the same type, does `guard ... else` leave, is `inout` allowed here)
belong to the type checker.

## Speed

Measured on 10 MB made from repeated copies of `tour.lang` (release build, best of 10, this container):

| Stage | Time | Speed |
|---|---|---|
| lex | about 105 ms | about 90 MB/s |
| parse | about 200 ms | about 50 MB/s |
| syntax checks | about 70 to 90 ms | about 120 MB/s |
| free the tree | about 100 ms | |

That is roughly 1.5 million lines per second for lexing, parsing and checking together. What was tried:

- Caching the current token in the parser: parse went from about 220 to about 200 ms.
- A bump allocator that never frees (as an experiment): about 20% faster on a single cold run, but memory grows
  without limit across runs and the gain is mostly page faults, so it is not worth the hack.
- Names are already "interned" in the cheapest way: an `Ident` is a span of the source, with no allocation. A real
  symbol table belongs to name resolution, where every use of a name must be compared.

What would help most is an index-based arena for tree nodes (one `Vec` per node kind, 4-byte ids instead of boxes).
It touches every consumer of the tree, so it waits until the type checker's needs are known. At the numbers above
the parser is not the bottleneck for the compile-speed target; the type checker and the backend will be.

## Not done yet

- Ordinary (non-doc) comments are skipped; the formatter needs them kept.
- Rules that need types, listed above.
