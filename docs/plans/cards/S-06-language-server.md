Id: S-06
Title: `pelago lsp`: a language server inside the compiler
Column: Review (L1 to L5 built; L6 left)
Order: -
Depends on: nothing blocking; S-05 is the Zed side of it
Size: L, in stages
Revisit when: the owner wants editor features beyond colours (errors as you type, hover, go to definition), or after the backend's N3.

# Status

The owner took it off the shelf ("tackle the tree-sitter and lsp first"). **Built:** `pelago lsp` (`compiler/src/lsp/`: `json.rs`, `analysis.rs`, `mod.rs`, `tests.rs`), no
external crates, 13 tests (JSON round trip and random input, framing, UTF-16 positions, every feature, a two-file workspace, the examples as a robustness run).
It offers diagnostics (all stages, cleared when fixed), hover (function signature, struct/enum shape, local and field types, or the type of the expression), go to
definition (across files of the package), find references, the document outline, semantic tokens (the highlighter's classes refined by the resolver: function,
method, parameter, property, struct, enum, enumMember, with `declaration`, `defaultLibrary` and `readonly` modifiers) and completion (keywords, declarations, the locals
of the enclosing item, and the members of the expression's type after a dot; not scope-exact).

**Left:** L6 (rename, formatting, code actions), signature help, doc comments in hover, completion of imports and of members from `impl`s for generic types, an incremental
checker (today every change re-checks the whole package), a VS Code client (needs the `vscode-languageclient` npm package: the owner's call), and the Zed hookup (S-05).
Hover shows `(local) x`, not `const`/`var`, for locals because the resolver does not record which it was.

# Original plan

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
| L6 | Rename, formatting (needs a printer for source, not just for the tree), code actions | code actions done: "did you mean" (read from the checker message) and "remove unused import" (a hint, found from tokens); rename and formatting later |

# Tests

A test driver feeds recorded JSON-RPC conversations to the server in memory and compares the replies (golden files, same habit as the parser's golden trees). The fuzz habit of the lexer applies: arbitrary input never crashes the server.

# Questions for the owner

1. Own JSON code inside the compiler (recommended, a few hundred lines) or an external crate?
2. Do we want a formatter (`pelago fmt`) before L6? It would also feed `format on save`.
