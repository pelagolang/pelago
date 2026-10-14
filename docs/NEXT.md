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
4. **Name resolution is done** (`docs/design/checker.md`, stage 1). **Stages 2 to 5 are done** (`langc check`): types, interfaces and bounds, exhaustiveness, value semantics (moves, exclusivity, `const` depth). The checker is complete for version 1 (stage 6 too: `converts`, `main`, `defer`; `become`, `linear`, `spawn`). **Lowering to IR is built** (Phase 2, `docs/design/ir.md` section 8): the whole checked language runs in the IR interpreter (`langc run`), without leaks. **Next:** a machine-code backend from the IR (or a faster interpreter first), real threads for tasks, `box<T>` operations in the checker, and `meta`. Choices VS1 to VS15 in `checker.md` await the owner's confirmation; VS3 (taking a part of an owning value needs `copy`) and VS10 (`linear`) are the biggest. Still deferred: `async`.

## The task board

Work is tracked as cards in `docs/plans/` (read `README.md` there, then `BOARD.md`). Next on track A: `A-01` cleanup, `A-02` box (needs six answers), `A-03` the ledger, `A-04` the IR optimiser. Track B: the name (`B-01`), then the highlighter (`B-03`).

## Open questions for the owner

- The final **name** (see `NAMES.md`, round 6). `langc` stays as the crate name until then.
- Whether to ship a **single tool** named after the language, with the compiler inside it.
