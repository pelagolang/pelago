# Where we stopped, and what comes next

## Done

- Design drafts in `docs/design/`: `basics`, `keywords`, `functions`, `types`, `switch`, `lexer`, `samples`.
- `ROADMAP.md` holds the decisions table and the phases.
- `compiler/` (Rust, no dependencies): keyword table, tokens, spans, and the lexer, with tests and CI.
- `examples/` holds source files that must lex cleanly.

## Next, in order

1. **The parser.** Tokens to syntax tree.
   - Define the syntax tree types first (one node type per construct in the design docs).
   - Recursive descent for declarations and statements, Pratt parsing for expressions.
   - Newline rules: ignore newlines inside brackets and where a line obviously continues.
   - Soft keywords: check `Keyword::from_ident` where a soft keyword is meaningful.
   - Split `>>`, `>=` and `>>=` when they close generic arguments.
   - Split string tokens into text and `#(...)` expression parts.
   - Ternary vs error-propagation `?`: spaces around the ternary `?`, none before the postfix one.
   - `!` / `!!` after a method name need to be adjacent to the name (compare token spans).
   - Parse everything in `examples/tour.lang`, then add golden-file tests.
2. **Collections**: decided in `docs/design/collections.md`, including builders. List patterns come next.
3. **Errors** are decided (`docs/design/errors.md`). **Concurrency** is decided (`docs/design/concurrency.md`, CC1 to CC9).
4. **Name resolution and the type checker.**

## Open questions for the owner

- The final **name** (see `NAMES.md`, round 6). `langc` stays as the crate name until then.
- Whether to ship a **single tool** named after the language, with the compiler inside it.
- Quick confirms at the end of `docs/design/switch.md` (N1 to N3).
