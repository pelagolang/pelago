Id: A-13
Title: Bump arenas: `arena`, `aref<T>`, reset in one store
Column: Doing
Order: 13
Depends on: A-02 (box), nothing in the backend
Size: M
Plan: [docs/design/arena.md](../../design/arena.md)

# What

An owned `arena` value with bump allocation, checked handles (`aref<T>`, 32-bit offset plus 32-bit epoch) and a one-store
`reset!`. A library type over `rt_alloc`, `rt_realloc` and `rt_free`, so the runtime period needs no change for it.

# Done means

- Stages AR1 to AR5 of the plan are merged (AR6, drop lists, only if the owner says yes to question 1).
- `bintrees` through an arena beats the `box` version (one run each, the number goes in the PR); `lexer` no longer builds a list in `chars()`.
- `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` are green; the leak check is clean.

# Progress

- AR1 plan: this card and `docs/design/arena.md`.
