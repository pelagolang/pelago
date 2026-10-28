# Handoff: modularize core + missing library features

Repo: pelagolang/pelago. Written 2026-10-28. All work stopped; nothing is running.

## Goal (Dennis, 2026-10-27)
Modularize the `core` package, add the features that killed the benchmarks (bigint, regex, JSON, float to int, byte types), make camelCase the default function naming (snake_case still accepted). Out of scope: speed work, libc/runtime work (file/stdin/args wait for the no-libc runtime period), coroutines (concurrency thread, #131).

## Merged
- #129 core split into modules (`compiler/core/core/*.pel`)
- #132 camelCase library (B8 in docs/design/basics.md, ROADMAP "Naming" row)
- #133 float to int, byte types

## Open PR
- #136 `claude/refactor/core-index-and-library-tests`: `core.pel` as an index file (like `index.ts`/`mod.rs`), library tests moved out of the source files into `compiler/core/tests/` and `compiler/std/tests/` (`.pel` + `.out`, `.stops` marker = must panic), run by `library_test_suites` in `compiler/src/lower/tests.rs`. Needs `origin/main` merged in again (main moved), CI green, then squash merge. Tick every PR-template checkbox, assign C-EO.

## Pushed branches without PRs (stacked, in this order)
1. `claude/fix/copy-of-empty-list` (c7cb286, based on the #136 branch): fixes `copy` of an empty list/string then `push!` crashing ("used an address outside memory"). Fix is in `compiler/src/lower/glue.rs` (`clone_body`: cap = max(len,1)); test `compiler/core/tests/copy_empty`.
2. `claude/feat/std-libs` (81f3cab) on top of 1. Three commits:
   - 3a086fd `std` package (loaded only when a program has `import std.<module>`; `STD_MODULES` in `package.rs`) plus `std.big` (bigint, base 1e9 limbs, Knuth D), `examples/programs/factorial.pel`, test `std_is_added_only_when_imported`
   - 7ca0e7e `std.json` plus exact `str.parseFloat` (exponents) and `pushChar`/`pushByte`
   - 81f3cab `std.regex` (Pike VM over bytes, leftmost-first, verified against Python `re` on 44 patterns)
   Plus `docs/design/std.md`.
   (Older local branches std-bigint/std-json/std-regex were superseded and never pushed.)

## Next steps
1. Merge `origin/main` into #136, wait for CI, squash merge.
2. Rebase the fix branch onto main, open a PR `fix(lower): ...`, merge.
3. Split `std-libs` into three PRs (std+bigint, json, regex) rebased on main (squash merges make stacked branches need rebasing/cherry-picking), or open one PR. Commits are already separated.
4. Use the PR template (`.github/pull_request_template.md`), all boxes ticked; Conventional Commit titles.

## Build and test (PowerShell works the same)
```
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test                              # about 100 s
cargo test --lib library_test_suites    # just the .pel suites
cargo run -- run <file-or-dir>
cargo run -- fmt compiler/std           # and: fmt --check examples
$env:BLESS=1; cargo test                # refresh golden trees (bash: BLESS=1 cargo test)
```
Note: main now runs native by default; the interpreter is behind `--interp` (#140).

## Known bugs and quirks
- Fixed only on the unmerged fix branch: copy of an empty list/string.
- `option.map` returns an option (quirk, untouched).
- Regex: `a{,2}` is treated as a literal (Python differs); empty match advances one byte.
- Language quirks: ordinary comments are `///`; `some(...)` is only a pattern; `to` rejects lossy conversions (use `rt_to_byte`/`rt_to_char`); taking a field/element needs `copy`; a module and its type with the same name are ambiguous (hence `json.value`, `big`/`bigint`).

## Open questions for Dennis
1. Rename `rt_*` internals to camelCase? The compiler matches the prefix in about 100 places, so they stay snake_case for now.
2. OK for types `parse_error`/`scope_handle` to stay snake_case?
3. JSON typed decoding needs `meta` reflection; defer?
4. Coroutines belong to the concurrency thread (#131/A-12). File/stdin/args wait for the runtime period.
5. Arena tests are inline Rust (other thread), not in the new suite layout; move them?
