Id: S-06
Title: `pelago lsp`: a language server inside the compiler
Column: Shelf
Order: -
Depends on: nothing blocking; S-05 is the Zed side of it
Size: L, in stages
Revisit when: the owner wants editor features beyond colours (errors as you type, hover, go to definition), or after the backend's N3.

# Parked on purpose (owner decision)

Shelved together with S-05. The notes below are the plan so that nobody has to rediscover it.

# Design

- **A subcommand of the one tool:** `pelago lsp` speaks the Language Server Protocol on standard input and output. No separate binary, no external dependency: a small JSON reader and writer and the JSON-RPC framing (`Content-Length` headers) are written in the compiler.
- **Every editor can use it**: Zed, VS Code, Neovim, Helix.
- **What already exists to build on:** every stage (lexer, parser, resolver, checker, values pass) reports diagnostics with a file and a span; `highlight.rs` classifies every byte; the resolver knows where every name is defined and used.
- **Known limit:** the checker works on a whole program and has no incremental mode. Fine for small projects; the plan for a large one is to cache per module (the package layer already knows the modules).

# Stages

| Stage | Feature | Built on |
|---|---|---|
| L1 | Transport, `initialize`, document sync (full text), **diagnostics** from lexer, parser, resolver, checker (errors and warnings) | `package.rs`, `Diagnostic` |
| L2 | **Semantic tokens** (colours that know types and functions) | `highlight.rs` classes |
| L3 | **Hover** (the type of an expression, the signature of a function) | typecheck results |
| L4 | **Go to definition**, find references | resolver |
| L5 | Document symbols (outline), signature help, completion of names in scope and of methods after a dot | resolver, types |
| L6 | Rename, formatting (needs a printer for source, not just for the tree), code actions | later |

# Tests

A test driver feeds recorded JSON-RPC conversations to the server in memory and compares the replies (golden files, same habit as the parser's golden trees). The fuzz habit of the lexer applies: arbitrary input never crashes the server.

# Questions for the owner

1. Own JSON code inside the compiler (recommended, a few hundred lines) or an external crate?
2. Do we want a formatter (`pelago fmt`) before L6? It would also feed `format on save`.
