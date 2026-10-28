# Handoff: narrow links and smaller binaries/data (thread "Shrink binaries and data with narrow links")

Repo: pelagolang/pelago. Owner: Dennis (GitHub C-EO), works on Windows/PowerShell. Read `CLAUDE.md` in the repo first.

## Goal
tcc-small executables and small data for Pelago. Add narrow relative links (1, 2 or 4 byte) and use byte-width types.

## Merged (all on main)
- #138 `perf(native): keep the top of the stack off huge pages`: removed the 2 MiB memory floor (`hello` 2072 to 1116 KiB). Files: `compiler/src/native/rt.rs`, `rt_a64.rs` (`STACK_SMALL`).
- #139 `docs(design): audit executable and data size, propose narrow links`: `docs/design/narrow-links.md`.
- #145 `docs(design): record narrow link decisions and plan card A-14`: decisions NL1 to NL4, card `docs/plans/cards/A-14-pool-and-links.md`, BOARD and ROADMAP rows.

## Open PRs / unpushed work
None. No local work to rescue.

## Audit findings (x86-64 Linux)
- Executables are already 2.6 to 20 KB (median 5.3 KB); almost all `.text`. Code-size levers are the sqrt/floor/ceil intrinsics (nbody 11.9 KB, software sqrt), not links. `kostya_bf_mandel` is 20 KB because of an 11.7 KB `.data` literal.
- `tcc` could not be installed in the container, so the tcc comparison is still missing (the bench harness has a tcc row).
- Byte types already pack: `[uint8]` is 1 byte per element (type names are `int8/uint8/int32/...`).
- Pointer-heavy data is the remaining cost: `box`/`obj` are 8-byte pointers, 16-byte block header, `bintrees` 8.2 MiB peak (about 48 bytes per node).

## Decisions (Dennis, 2026-10-27)
NL1 pool-relative links (not self-relative); NL2 names `pool` and `link` (he marked the question answered without a different name); NL3 width always written, e.g. `link<node, uint16>`; NL4 pool is its own card (A-14), after the bump arenas of A-13 (`docs/design/arena.md`).

## Next steps
1. Let A-13 (bump arenas, `aref<T>`) land, or reuse its block code.
2. A-14 P1: `pool<T, W>` (`add!`, `[]`, `remove!`, `len`, stops when full at 2^W slots).
3. A-14 P2: `link<T, W>` as a narrow integer in the IR, `none` = all ones, resolved only through its pool, bounds-checked. Check struct layout (tag byte + two 4-byte links = 12 bytes).
4. A-14 P3: bench twins (`bintrees`, `graph`, `lru`), judged by peak memory.
New syntax needs: parser, `ast_print.rs`, `visit.rs`, `check.rs`, `examples/tour.pel`, a parser test, `BLESS=1 cargo test`.

## Build and test (PowerShell)
```
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run -- build -o out.exe bench\programs\hello\pelago.pel
```
Measure memory with a vfork/wait4 wrapper (`ru_maxrss`); do not read it from a forking parent (inflated). Avoid full benchmark runs unless asked.

## Rules
Branches `claude/<type>/<name>` (or your own author prefix); Conventional Commits for titles and commits; PR template with every box ticked (N/A marked); no external crates without asking.

## Open questions for Dennis
None blocking. Confirm `pool`/`link` names if he cares.
