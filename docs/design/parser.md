# Parser (draft 1)

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

## Choices to confirm (P1 to P10)

**P1. Statements at the top level of a file.** `tour.lang` has them, so the file node holds statements and
items alike. A later pass would reject executable statements outside `fn`s in a real module. Fine?

**P2. Import forms.** `import std.io`, `import a, b from std.io`, and `use` as another spelling. There is no
`as` rename yet. Keep, or add `import std.io as fmt`?

**P3. No struct literals in conditions.** `if p == point { x: 1 } { }` is ambiguous, so a literal there needs
parentheses (as in Rust). Same for `while`, `unless`, `guard`, `for` and `switch` heads. Fine?

**P4. Bitwise operators bind tighter than comparison** (Rust and Go, not C), so `a & b == c` is `(a & b) == c`.
Keep?

**P5. `to` binds between `+` and the comparisons**: `a + b to float` is `(a + b) to float`, and
`x to float < 3.0` is `(x to float) < 3.0`. Keep?

**P6. Ranges bind tighter than comparison and looser than `to`/`+`**, so `x in 0..n + 1` and
`0..10 step 2` read naturally. Keep?

**P7. `break` and `yield` may carry a value** (`const r = loop { break 5 }`). Is `loop` meant to be an
expression with a value?

**P8. Loop labels** (`outer: for ...`, `break outer`) are not parsed. Do you want them? A syntax would be needed.

**P9. Closure return types** (`(a: int) -> int ~> a + 1`) are not parsed; a closure body is one expression or a
block. Fine for now?

**P10. `T??`** cannot be written because `??` is one token; write `(T?)?`. Fine, since a nested option is rare?

## Not done yet

- Doc comments are skipped, not attached to declarations. The formatter will need them kept.
- The checker rules for things the parser accepts: `++` only as a statement, `inout` / `sink` only where
  modes are allowed, one `...` per list pattern (already reported), and so on.
- Speed: a first measurement of lexing plus parsing is about 25 MB/s on a repeated copy of `tour.lang`
  (the lexer alone is about 75 MB/s on the same text). Expect a good gain from an arena and interned names.
