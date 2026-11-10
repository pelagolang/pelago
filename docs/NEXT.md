# Where we stopped, and what comes next

> Cycle rhythm and rules: [docs/process/workflow.md](process/workflow.md). The retro of each two-week cycle (ten lines: shipped, broke, next) is added at the top of this file. The sections below are older and are refreshed at the first retro; the live work list is `docs/plans/BOARD.md`.

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
4. **Name resolution is done** (`docs/design/checker.md`, stage 1). **Stages 2 to 5 are done** (`pelago check`): types, interfaces and bounds, exhaustiveness, value semantics (moves, exclusivity, `const` depth). The checker is complete for version 1 (stage 6 too: `converts`, `main`, `defer`; `become`, `linear`, `spawn`). **Lowering to IR is built** (Phase 2, `docs/design/ir.md` section 8): the whole checked language runs in the IR interpreter (`pelago run`), without leaks. **Next:** a machine-code backend from the IR (or a faster interpreter first), real threads for tasks, and `meta`. Choices VS1 to VS15 in `checker.md` await the owner's confirmation; VS3 (taking a part of an owning value needs `copy`) and VS10 (`linear`) are the biggest. Still deferred: `async`.
   **The braid is built** (card S-09, Done: `docs/design/alternating.md` section 10; PRs 235 and 236). Also built since: `strand<T>` and `strand_mut<T>` views (with `for` over them; `evens`/`odds` still copy), `asPairs`/`asSlots`, the `@derive(toJson)` encoder, and pair methods for a cycle of three types. If wanted next: cycles of four or more types, and calls on built-in types in general (`T.f()`, as in `braid.zip`).

## The task board

Work is tracked as cards in `docs/plans/` (read `README.md` there, then `BOARD.md`). Cards A-01 to A-08 and B-03 are done (box, the ledger, the optimiser, meta, soa, async at stage 0, the highlighter). The machine-code backend (card A-09) runs: stages N1 to N5 and the Windows target are done (`pelago build`). Card A-11 (native speed) is under way: division by any constant (high multiply), range-proven overflow checks and a leaner generator brought collatz from 0.30 s to 0.09 s (C `-O2` 0.07 s). **Next:** the work items in `docs/perf/a11-work-items.md` (induction-variable ranges, real linear-scan allocation, memory operations, benches, the A-09 leftovers), written to be split across parallel threads. Then the shelf (S-01 maps, S-02 threads, S-04 async suspension). Parked on purpose: the Tree-sitter grammar and Zed extension (S-05) and the language server (S-06); the two empty repositories exist under `pelagolang`.

## Open questions for the owner

- Whether to ship a **single tool** named after the language, with the compiler inside it.
