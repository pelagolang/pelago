# lang

A self-hosting systems language, name to be decided. Compiled natively with its own backend, no GC and no runtime, memory safe through value semantics, with a TypeScript-like feel.

- [ROADMAP.md](ROADMAP.md): vision, decisions and phases
- [docs/design/samples.md](docs/design/samples.md): sample programs in the imagined syntax
- [docs/design/basics.md](docs/design/basics.md), [keywords.md](docs/design/keywords.md), [functions.md](docs/design/functions.md), [types.md](docs/design/types.md): language design drafts
- [docs/design/switch.md](docs/design/switch.md), [collections.md](docs/design/collections.md), [errors.md](docs/design/errors.md), [concurrency.md](docs/design/concurrency.md), [list-patterns.md](docs/design/list-patterns.md), [parser.md](docs/design/parser.md), [modules.md](docs/design/modules.md), [memory.md](docs/design/memory.md), [lexer.md](docs/design/lexer.md): more design notes
- [docs/NEXT.md](docs/NEXT.md): where we stopped and what comes next
- [NAMES.md](NAMES.md): name brainstorm

Status: **Phase 0 (design on paper) with the first Phase 1 code started.**
`compiler/` holds the stage 0 compiler in Rust: so far the keyword table, tokens, spans, the lexer, the syntax tree and the parser.
`examples/` holds sample source files.

```
cargo test
cargo run -- keywords
cargo run -- lex examples/tour.lang
cargo run -- parse examples/tour.lang
```
