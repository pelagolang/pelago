# Handoff: research on replacing the heap allocator

## Goal
Dennis asked (2026-10-27) whether Pelago can reserve and use memory more efficiently, and whether to ditch the heap allocator and heap. Output wanted: a design recommendation (research only, no code).

## Done and merged
- PR #143 (merged, squash 48c6eda): `docs/design/heap.md` in pelagolang/pelago. Copy: `/mnt/project-files/design/heap-replacement.md`.
- Findings in it:
  - The 2 MiB floor came from `MADV_HUGEPAGE` on the 1 GiB start-up stack (found by the narrow-links thread; fix in PR #138; `hello` 2072 -> 1116 KiB, C 1300).
  - Allocator waste: doubling size classes, 16-byte header per block, `rep stosb` clearing on reuse, 2 MiB rounding of large blocks. Code: `compiler/src/native/rt.rs` (x86-64) and `rt_a64.rs` (AArch64 copy).
  - A pure bump-stack design (Goose style) breaks on returned values (`make() -> tree`) and out-of-order frees, so recommend a hybrid with no libc-style heap.
- Recommended design, stages H0 to H6 (all in the doc): reserve one big address range and commit on demand; 64 KiB spans per size class with a one-byte class table (no block headers); finer size classes; no zeroing on reuse; huge pages off by default; whole-program specialisation (no free anywhere -> pure bump; no allocation -> no allocator); arenas (A-13) on their own reserved ranges; escape analysis for local temporaries; per-worker sub-ranges and a task stack pool; later write the allocator in Pelago.
- Expected effects are estimates, not measurements: `bintrees` about 2x behind C instead of 4.7x; `mapwork` at most 10 to 20%; `strsplit` 1.3 to 2x; `lexer` unchanged (needs lazy `str.chars()`, arena card AR5).

## Open PRs / branches / unpushed work
None from this thread. Related threads: PR #138 (stack floor fix) and #139 (size audit, narrow links design) from the narrow-links thread; arena work (A-13, `docs/design/arena.md`) from the arena thread. Check their state on GitHub.

## Next steps (not started)
1. Dennis answers the 5 questions at the end of `docs/design/heap.md` (literal vs libc-style no-heap; reservation size; zeroing rule; where the allocator is written; huge pages).
2. Record decisions in `docs/design/heap.md` and the `ROADMAP.md` decisions table; add plan cards for H1 and H2 (`docs/plans/cards/`, `docs/plans/BOARD.md`).
3. H1: region heap with the same `Alloc/Free/Realloc` ABI in `rt.rs`, then `rt_a64.rs`. One PR, name only the `bintrees` row.
4. H2: allocator specialisation by whole-program facts (`free_used` already exists).

## Build and test (PowerShell)
```
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run -- build -o hello.exe examples\hello.pel
```
Rules: Conventional Commits for titles and messages; branches `<author>/<type>/<short-name>`; PR template at `.github/pull_request_template.md` with every box ticked; no external Rust dependencies; do not run the full benchmark suite (`python3 bench/harness/run.py <name>` for a single row only).

## Known risks and caveats
- Effect numbers are unmeasured.
- `RLIMIT_AS` counts reservations and strict overcommit can refuse them: reserve in a loop, halving on refusal.
- The ChatGPT text Dennis pasted was about compile time (63 ms vs 6 ms for `base64`), not runtime memory; no thread was started for it.
