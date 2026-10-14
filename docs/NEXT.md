# Where we stopped, and what comes next

## Done

- Design drafts in `docs/design/`: `modules`, `checker`, `basics`, `keywords`, `functions`, `types`, `switch`, `collections`, `errors`, `concurrency`, `list-patterns`, `memory`, `ir`, `lexer`, `parser`, `samples`.
- `ROADMAP.md` holds the decisions table and the phases.
- `compiler/` (Rust, no dependencies): keyword table, tokens, spans, the lexer, the syntax tree and the parser, with tests and CI.
- `examples/` holds source files that must lex cleanly.

## Next, in order

1. **The parser** and the syntax checks are written and decided (`docs/design/parser.md`). Left there: ordinary comments kept for
   the formatter, and an index-based arena for tree nodes (when the type checker's needs are known).
   **Imports and modules** are decided (`docs/design/modules.md`) and implemented.
2. **Collections** (including builders) and **list patterns** are decided (`collections.md`, `list-patterns.md`). Revisit `sink` list patterns once the checker exists.
3. **Errors** are decided (`docs/design/errors.md`). **Concurrency** is decided (`docs/design/concurrency.md`, CC1 to CC9).
4. **Name resolution is done** (`docs/design/checker.md`, stage 1). **Stages 2 to 5 are done** (`langc check`): types, interfaces and bounds, exhaustiveness, value semantics (moves, exclusivity, `const` depth). The checker is complete for version 1 (stage 6 too: `converts`, `main`, `defer`; `become`, `linear`, `spawn`). **Next: lowering to IR** (Phase 2; IR1 to IR8 decided, step 1 built: `compiler/src/ir/` types, printer, verifier). Choices VS1 to VS15 in `checker.md` await the owner's confirmation; VS3 (taking a part of an owning value needs `copy`) and VS10 (`linear`) are the biggest. Still deferred: `async`.

## Open questions for the owner

- The final **name** (see `NAMES.md`, round 6). `langc` stays as the crate name until then.
- Whether to ship a **single tool** named after the language, with the compiler inside it.
