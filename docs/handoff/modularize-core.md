# Handoff: modularize core + missing library features

Repo: pelagolang/pelago. Written 2026-10-28. All work stopped; nothing is running.

## Goal (Dennis, 2026-10-27)
Modularize the `core` package, add the features that killed the benchmarks (bigint, regex, JSON, float to int, byte types), make camelCase the default function naming (snake_case still accepted). Out of scope: speed work, libc/runtime work (file/stdin/args wait for the no-libc runtime period), coroutines (concurrency thread, #131).

## Merged
- #129 core split into modules (`compiler/core/core/*.pel`)
- #132 camelCase library (B8 in docs/design/basics.md, ROADMAP "Naming" row)
- #133 float to int, byte types

## Status (updated 2026-11-06): all merged
- #136 core.pel index + library test suites (core/tests, std/tests)
- #169 fix(lower): copy of an empty list/string (test `core/tests/copy_empty`)
- #170 `std` package + `std.big`; #171 `std.json` + exact `parseFloat`; #172 `std.regex`
No open PRs and no unpushed work remain for this thread. The old branches (`claude/fix/copy-of-empty-list`, `claude/feat/std-libs`, `claude/refactor/core-index-and-library-tests`) are merged or superseded and can be deleted.

## Next steps (optional follow-ups)
1. Decide the open questions below (mainly `rt_*` camelCase).
2. Typed JSON decoding via `meta` reflection; coroutines (concurrency thread); file/stdin/args in the runtime period.

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
