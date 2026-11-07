Id: A-14
Title: Pools and narrow links: smaller data for pointer-heavy programs
Column: Doing
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

# Progress

- P1 and P2 as a library, `compiler/core/core/pool.pel`: `pool<T, W>` (`add!`, `get`, `set!`, `remove!`, `clear!`, `len`, `noLink`, `isNone`) and `link<T, W>`, a plain struct holding a `W`. The all-ones number means no slot, so a pool holds at most `2^bits - 1` slots; it stops with "pool full". `W` cannot be converted generically with `to`, so the pool bounces the slot number through an 8-byte buffer (`rt_write`/`rt_read`, little-endian). `none` is a keyword, hence `noLink`. A node of two `uint16` links and an `int32` is 8 bytes. Tests in `lower/tests.rs` (interpreter and native). A slot is as big as the value (or the link, if bigger), with no rounding; a free slot keeps the next free number in its first `W` bytes (done in the tight-slots PR; `slotBytes()` shows it).
- P3: `bench/programs/bintrees/pelago_pool.pel` (same output, 1310700). Peak memory by `ru_maxrss`: box 8192 KiB, arena 4096 KiB, pool 1116 KiB (floor).
- Free optional link, parked: `option<link<T, W>>` is a tag plus the link (4 bytes for a `uint16` link, 2 for the link alone). A niche (all ones means `none`) would save the tag, but every place that reads or writes an `option` tag (constructors, `switch`, `is some`, `?`, drop and clone glue, `enum_info` in `lower/ty.rs`) would need a second path, for 1 to 3 bytes per optional link. `noLink` and `isNone` give the same bytes today in a plain field. Revisit if a benchmark with many optional links shows it.
- Width check (done): `pool()` stops at run time unless `W` is `uint8`, `uint16` or `uint32` (`rt_is_small_uint<W>()`); a compile-time bound needs generic constraints on a type's kind, which the checker does not have.
