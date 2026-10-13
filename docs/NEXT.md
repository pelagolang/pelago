# Where we stopped, and what comes next

## Done

- Design drafts in `docs/design/`: `basics`, `keywords`, `functions`, `types`, `switch`, `collections`, `errors`, `concurrency`, `list-patterns`, `memory`, `lexer`, `parser`, `samples`.
- `ROADMAP.md` holds the decisions table and the phases.
- `compiler/` (Rust, no dependencies): keyword table, tokens, spans, the lexer, the syntax tree and the parser, with tests and CI.
- `examples/` holds source files that must lex cleanly.

## Next, in order

1. **The parser** is written (`docs/design/parser.md`, `compiler/src/parser/`). Open: the choices P1 to P10 in that page,
   doc comments kept in the tree for the formatter, and an arena for speed.
2. **Collections** (including builders) and **list patterns** are decided (`collections.md`, `list-patterns.md`). Revisit `sink` list patterns once the checker exists.
3. **Errors** are decided (`docs/design/errors.md`). **Concurrency** is decided (`docs/design/concurrency.md`, CC1 to CC9).
4. **Name resolution and the type checker** are next: modules and scopes first, then types, then the value-semantics checker.

## Open questions for the owner

- The final **name** (see `NAMES.md`, round 6). `langc` stays as the crate name until then.
- Whether to ship a **single tool** named after the language, with the compiler inside it.
- Quick confirms at the end of `docs/design/switch.md` (N1 to N3).
