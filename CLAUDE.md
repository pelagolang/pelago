# CLAUDE.md

Guide for working in this repository.

## What this is

A new self-hosting systems language. The name is undecided: the working name is `lang`, and the
compiler crate is `langc`. We are in Phase 0 (design on paper) with the first Phase 1 code started.
Read `README.md`, then `ROADMAP.md` (vision, decisions table, phases), then `docs/NEXT.md` (where we
stopped and what comes next).

## Commands

```
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run -- keywords            # list keywords (hard and soft)
cargo run -- tokens              # list operators
cargo run -- lex <file>          # print tokens, report lexer errors (`-` reads stdin)
cargo run -- lex-bench <file>    # time the lexer alone
cargo run -- parse <file>        # print the syntax tree, report errors
cargo run -- parse-bench <file>  # time lexing plus parsing
cargo run -- check <path>        # parse, resolve names and check types of a file or a directory
cargo run -- ir <path>           # check, lower to the IR, verify and print it
BLESS=1 cargo test               # rewrite compiler/tests/golden/*.ast after an intended syntax change
```

CI (`.github/workflows/ci.yml`) runs the first three. Keep them green before pushing.

## Layout

- `compiler/src/`: the stage 0 compiler in Rust (`keyword.rs`, `token.rs`, `span.rs`, `lexer.rs`, `ast.rs`, `ast_print.rs`, `visit.rs`, `check.rs`, `package.rs`, `parser/`, `resolve/`, `types.rs`, `typecheck/`, `values/`, `ir/`, `lower/`, `main.rs`; the library `compiler/core/core.lang`).
- `compiler/tests/`: integration tests. Every file in `examples/*.lang` must lex and parse without errors and match its golden tree.
- `examples/`: sample source files. Keep `tour.lang` in step with the syntax decisions. `examples/programs/` holds real programs that must type check cleanly.
- `docs/design/`: design drafts (`basics`, `keywords`, `functions`, `types`, `switch`, `collections`, `errors`, `concurrency`, `list-patterns`, `lexer`, `parser`, `modules`, `checker`, `ir`, `memory`, `samples`).
- `ROADMAP.md`: the decisions table and the phases. `NAMES.md`: name brainstorm.

## Keeping things in step

- `docs/design/keywords.md` and `compiler/src/keyword.rs` describe the same list. Change both together.
- A new operator needs an entry in the `puncts!` table in `token.rs` **and** the match in `Lexer::punct`
  in `lexer.rs`. The test `every_punctuation_lexes_alone` enforces this.
- New syntax needs: the parser (`parser/`), the printer (`ast_print.rs`), the tree walker (`visit.rs`), any syntax rule (`check.rs`), a line in `examples/tour.lang`, a test in
  `parser/tests.rs`, and `BLESS=1 cargo test` to refresh the golden trees (review the diff).
- When a decision is made, record it in the relevant design doc and in the `ROADMAP.md` decisions table
  in the same commit.
- Words we dropped must not come back (`class`, `match`, `when`, `extern`, `comptime`, `inline`, word
  operators, exceptions, `null`). A test guards some of them.

## Code conventions

- Rust 2021, **no external dependencies**. Compile speed is a project goal, so ask before adding one.
- The lexer never panics and never returns a failure: bad input becomes an error token plus a `LexError`.
  Keep its invariants and the random-input test passing.
- Hard keywords are reserved everywhere. Soft keywords (`from`, `to`, `step`, `it`, `type`, `init`, ...) are
  identifiers to the lexer and are checked by the parser where they matter. Common names stay soft.
- Tokens stay small (at most 16 bytes) and never store their text; slice the source with the span.
- Match the surrounding style: module docs (`//!`) explain the rules, tests sit in the same file.

## Language decisions to respect

The full list is in `ROADMAP.md`. In short: compiled with our own backend, no GC, no runtime;
value semantics with `let` / `inout` / `sink` access modes (`inout` and `sink` repeated at the call site,
`c.bump!()` changes and `c.close!!()` consumes a receiver); no exceptions (`result`, `option`, `?`);
no null (`none`); symbol operators only; `fn`, `const`, `var`; lowercase type names; `obj` with
inheritance written `:`; one `switch` for pattern matching; `~>` closures; `#name` / `#(expr)` in strings;
`meta` for compile-time code; `pub` / `priv`.

## How we work with the owner

- Propose with a recommendation and the trade-offs, then let the owner decide. Drafts end with numbered
  questions. Record the answers in the docs.
- The owner wants a language with its own voice, not a copy of another one, but also familiarity where it
  helps. Be honest about costs, push back with reasons once, then follow the decision.
- For names, search the web for clashes before proposing, and avoid nature-themed names.
- Keep chat answers short and well organised; the owner reads the docs for detail.
- The owner is budget-conscious. Do not start large work (for example the parser) unless asked.

## Git

- Develop on `claude/programming-language-design-psjufd`. PR #1 is merged into `main`; later work needs a new
  pull request, and only when the owner asks for one.
- The owner switches GitHub's default branch (Settings, General, Default branch); the tools here cannot.
