Id: A-14
Title: Pools and narrow links: smaller data for pointer-heavy programs
Column: Next
Order: 14
Depends on: A-13 (bump arenas: `pool` reuses its block and reset); byte and narrow integer types (done)
Size: L, in two stages
Plan: [docs/design/narrow-links.md](../../design/narrow-links.md)

# What

Decisions NL1 to NL4 of the plan: `pool<T, W>` owns elements in one growable block, `link<T, W>` is a 1, 2 or 4 byte index into it (`W` is `uint8`, `uint16` or `uint32`, always written).

# Stages

A-13's `aref<T>` is an 8-byte handle (offset plus epoch) into an untyped bump arena. A `pool<T, W>` is the typed, fixed-element-size sibling whose handle is narrow, so it shares A-13's block code but not its handle.

- **P1 pool allocator.** `pool<T, W>` with `add!`, `[]`, `remove!` (free list), `len`; stops when full at `2^W` slots; freed in one go when the pool goes out of scope. Library type plus checker rules, IR lowering, interpreter and native.
- **P2 links.** `link<T, W>` as a narrow integer in the IR, `none` as the all-ones value so `option<link<T, W>>` is free, resolved only through its pool (`pool[link]`, bounds-checked). Layout check: a struct of a tag byte and two 4-byte links takes 12 bytes.
- **P3 bench twins.** `bintrees`, `graph`, `lru` with links, judged by peak memory (before: `bintrees` 8.2 MiB).

# Done means

- Tests in the interpreter and natively (x86-64, aarch64, Windows) for each stage; `fmt`, `clippy`, `cargo test` green.
- `examples/tour.pel` and the goldens updated for the new types.
- The plan page and ROADMAP decisions row stay in step.
