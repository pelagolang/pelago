# Handoff: runtime design and namespaces thread (2026-10-28)

## Goal
Answer Dennis's questions about the runtime and namespaces in the Pelago repo (pelagolang/pelago). Research only: **no code, branches or PRs were made**. Nothing is merged, nothing is open, nothing is unpushed.

## Deliverable
`/mnt/project-files/reviews/runtime-and-namespaces-audit.md`: audit of the runtime (designed/implemented/missing), how modules and imports work (tested), gaps, next steps, benchmark notes.

## Decisions from Dennis
- No `libc` whatsoever, ever (raw syscalls / own OS layer).
- A panic stops the program immediately: no unwinding, `defer`/`deinit` do not run. Not yet written into the design docs (add to `docs/design/errors.md` and `concurrency.md`).
- Runtime work should be one dedicated period, not baby steps.
- Benchmarks wanted: C++ and tcc columns, plus a suite as mature as aardappel/goose `bench/results.md` (I never read it). Note: tcc is fast to compile but slow to run, so use it in a compile-time benchmark; C++ twins mostly test `std::` containers.

## Next steps (in order)
1. Write `docs/design/runtime.md`: start-up, panic/exit-code policy, stdio and files (`io_error`), args/env, time/random, allocator contract, FFI ABI without libc, real threads (card S-02), the `core` vs `std` split. Record decisions in `ROADMAP.md`.
2. Fix `fn main(args: [string])`: it passes the checker but `pelago run` stops with `expected an address`. Add `eprint`, `exit(code)`, and exit status from `main`.
3. Implement I/O (stdin, files, stderr) as raw syscalls in `native/rt.rs`, `rt_a64.rs` and the interpreter, with `core.pel` wrappers.
4. Give `import std.x` a meaning (a bundled package next to `core`); add `[package]`/`[dependencies]` to `pelago.toml`.
5. Make `pelago run/check <file>` find its project root (single file currently loses sibling imports such as `import .a`); same for the language server.
6. Resolve the package-name vs module-name clash (package `main` vs `main.pel`); show the "local beats import" warning in `check`.
7. Benchmarks: add C++ (`g++ -O2`) twins, a compile-time suite (pelago vs tcc vs gcc vs rustc), more programs, generated `results.md`. Files: `compiler/benches/compare.sh`, `compiler/benches/c/`, `compiler/benches/rs/`.
8. Then cards S-02 (real threads) and S-01 (map removal O(n)) in `docs/plans/cards/`.

## Build and test (PowerShell)
```
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run -- check .\examples
cargo run -- run .\examples\hello.pel
```
Read first: `CLAUDE.md`, `README.md`, `ROADMAP.md`, `docs/NEXT.md`, `docs/design/backend.md`, `docs/design/modules.md`.

## Repo conventions
Branches `<author>/<type>/<short-name>`; Conventional Commits for commits and PR titles; PR template with every box ticked; no external crates; keep `examples/*.pel` compiling.

## Open questions for Dennis
- `std` package layout and what stays in the `core` prelude.
- Exit codes beyond 1; panic inside a task (decided: stops the program, per CC1).
