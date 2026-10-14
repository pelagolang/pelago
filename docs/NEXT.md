# Where we stopped, and what comes next

## Done

- Design drafts in `docs/design/`: `basics`, `keywords`, `functions`, `types`, `switch`, `collections`, `errors`, `concurrency`, `list-patterns`, `memory`, `lexer`, `parser`, `samples`.
- `ROADMAP.md` holds the decisions table and the phases.
- `compiler/` (Rust, no dependencies): keyword table, tokens, spans, the lexer, the syntax tree and the parser, with tests and CI.
- `examples/` holds source files that must lex cleanly.

## Next, in order

1. **The parser** and the syntax checks are written and decided (`docs/design/parser.md`). Left there: ordinary comments kept for
   the formatter, and an index-based arena for tree nodes (when the type checker's needs are known).
   **Imports** are decided except IM3 (what names a module) and IM4 (relative paths) in `docs/design/modules.md`; IM3 unlocks name resolution and the type checker.
2. **Collections** (including builders) and **list patterns** are decided (`collections.md`, `list-patterns.md`). Revisit `sink` list patterns once the checker exists.
3. **Errors** are decided (`docs/design/errors.md`). **Concurrency** is decided (`docs/design/concurrency.md`, CC1 to CC9).
4. **Name resolution and the type checker** are next: modules and scopes first, then types, then the value-semantics checker.

## Open questions for the owner

- The final **name** (see `NAMES.md`, round 6). `langc` stays as the crate name until then.
- Whether to ship a **single tool** named after the language, with the compiler inside it.
- IM3 and IM4 in `docs/design/modules.md`.
