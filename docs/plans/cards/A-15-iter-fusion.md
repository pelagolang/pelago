Id: A-15
Title: Lazy iterators by fusion in `for` loops
Column: Doing
Order: 15
Depends on: A-13 (the `chars()` loop, PR 163)
Size: M
Plan: [docs/design/iter.md](../../design/iter.md)

# What

`for x of <source>(.map(f) / .filter(p))*` becomes one loop that pulls from the source in place, so no temporary lists are built.
Stages IT1 to IT5 are in the plan; IT1 (`chars()`) is merged.

# Done means

- IT2 to IT3 merged with tests in the interpreter, optimiser and native; `strsplit` measured once.
- Elements are processed one at a time; `break` stops the source; overflow traps unchanged.
- `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` green.

# Progress

- IT1 done in PR 163.
- Plan written (this card and `docs/design/iter.md`).
