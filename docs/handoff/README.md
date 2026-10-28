# Pelago handoff index

For an AI agent (e.g. Gemini) taking over stopped agent threads. Facts come only from the handoff files in this folder. Read the repo's `CLAUDE.md` first.

## Project summary

Pelago is a programming language with its own compiler, written in Rust, in the GitHub repo `pelagolang/pelago`. It writes native executables itself (x86-64 and AArch64; Linux, Windows, macOS), with an IR optimiser, a machine IR (MIR) in progress, and a core library in `.pel` files under `compiler/core`. The owner is Dennis (GitHub C-EO), who works on Windows/PowerShell. The standing goal is that every benchmark runs at or below both gcc -O2 and Rust -O while keeping exact overflow traps and using no benchmark-specific tricks.

Conventions to keep:
- Branches are named `<author>/<type>/<short-name>`, for example `claude/fix/name`. No random suffixes.
- PR titles and commit messages use Conventional Commits, `type(scope): summary`.
- Use `.github/pull_request_template.md` with every checkbox ticked (N/A ticked and marked N/A).
- Never drop overflow traps.
- No libc in the runtime, ever (raw syscalls or an own OS layer). A panic stops the program with no unwinding, so `defer` and `deinit` do not run.
- Avoid running full benchmarks. Dennis has little usage left. Run a single row only when asked, and do not clone upstream benchmark repos.
- No external Rust dependencies. Comments in `.pel` are `///`.
- Before pushing, run `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test`.

## Handoff files

| File | Status |
|---|---|
| `arena-allocator.md` | Bump arena (card A-13) is merged (#135, #141, #149). Next: ask Dennis the 5 questions in `docs/design/arena.md`, then inline fast paths (AR4). |
| `base64-uint8-experiment.pel` | Experiment code, not a handoff. A base64 encode/decode with `[uint8]` lists (0.183 s vs 0.333 s with `[int]`). It uses `rt_to_byte` because there is no int-to-byte narrowing. |
| `compile-time-latency.md` | `--timings` merged (#144). `weight_live` fix is on an unmerged branch and #146 needs a status check. Next: avoid cloning the Func in `ivopt` candidates, share analyses across passes. |
| `concurrency.md` | C0 to C3b merged (#128, #150, #151). Nothing open. Next: wait queues and timeouts (rest of C3), then C5 atomics with C6. C4, C6 and C7 are blocked on questions or the runtime period. |
| `heap-research.md` | Research done, design doc merged (#143, `docs/design/heap.md`). No code. Waiting on Dennis's 5 questions before H1 (region heap). |
| `intrinsics-and-base64.md` | sqrt/floor/ceil intrinsics merged (#142). base64 investigated, no code changed. Waiting on Dennis to choose the int-to-byte narrowing spelling. |
| `loop-ranges.md` | Optimiser and native speed (card A-11). Merged #61 to #87, nothing open. Only gap is fib38 (1.1x to 1.4x of C); the rest is at or below C. Next: publish the `--checked` table, add more benchmarks. |
| `modularize-core.md` | Core split, camelCase and float/byte types merged (#129, #132, #133). **PR #136 is open**, plus two unmerged branches (below). |
| `narrow-links.md` | Stack floor fix (#138) and size audit and decisions (#139, #145) merged. Nothing open. Next: A-14 `pool<T, W>` and `link<T, W>`, after A-13. |
| `runtime-design-namespaces.md` | Research only; the audit is at `/mnt/project-files/reviews/runtime-and-namespaces-audit.md`. No code or PRs. Next: write `docs/design/runtime.md`, fix `fn main(args)`, implement I/O. |
| `w4-machine-level-ir.md` | MIR M0 to M3, M5a, M6a merged (#116 to #118, #152, #155, #156). MIR is off by default (`PELAGO_MIR=1`). Next: M5b (AArch64 floats and vectors), M6b (x86 rows over +5%), then M4 (make MIR the default). |

## Open PR

- **#136** `claude/refactor/core-index-and-library-tests`: `core.pel` becomes an index file, and library tests move to `compiler/core/tests/` and `compiler/std/tests/`. It needs `origin/main` merged in again, CI green, then a squash merge. Tick every PR-template checkbox and assign C-EO.

## Unmerged branches

- `claude/perf/ivopt-analysis-reuse` (pushed, no PR; from `compile-time-latency.md`): the same analysis sharing as PR #146 (it will conflict) plus the `weight_live()` fix. Measured base64 optimiser 58 to 38 ms with an identical binary. After #146 merges, rebase, keep only the `weight_live` change and open `perf(opt): skip a second dead-code pass when weighing ivopt candidates`. PR #146 (`claude/perf/compile-time`) was not confirmed merged, so check it.
- `claude/fix/copy-of-empty-list` (c7cb286, pushed, no PR, based on the #136 branch): fixes `copy` of an empty list or string followed by `push!` crashing. Fix in `compiler/src/lower/glue.rs`, test `compiler/core/tests/copy_empty`. Rebase onto main and open `fix(lower): ...`.
- `claude/feat/std-libs` (81f3cab, pushed, no PR, on top of the fix branch): `std` package, `std.big`, `std.json`, `std.regex` and `docs/design/std.md`. The three commits are already separate (3a086fd, 7ca0e7e, 81f3cab). Either open three PRs or one.
- Also, a local branch `claude/perf/base64-runtime` was created with no commits (`intrinsics-and-base64.md`).

## Open questions for Dennis

Arena (`arena-allocator.md`)
1. Five questions at the end of `docs/design/arena.md`: plain types only in v1 with drop lists later; mark/release; handle width; names; ambient arena for `box`. Then reconcile with `docs/design/heap.md`.
2. Is AR6 (drop lists, so non-plain types work) wanted at all?

Heap (`heap-research.md`), five questions at the end of `docs/design/heap.md`
3. Literal no-heap or libc-style no-heap?
4. Reservation size.
5. Zeroing rule.
6. Where the allocator is written.
7. Huge pages.

Concurrency (`concurrency.md`, plan section 9)
8. Stackful tasks as the model?
9. Async on tasks?
10. Unwinding for dropped coroutines and cancelled tasks?
11. Abandon detached tasks at the end of `main`?
12. 256 KiB default task stack?

Language and library
13. Spelling for narrowing int to byte or int8..int32. Recommendation: `n.wrapToByte()` and `n.toByteChecked()` returning `option`.
14. Rename `rt_*` internals to camelCase? (They stay snake_case for now.)
15. May the types `parse_error` and `scope_handle` stay snake_case?
16. Should JSON typed decoding, which needs `meta` reflection, be deferred?
17. Should the arena tests, which are inline Rust, move into the new library suite layout?
18. `std` package layout, and what stays in the `core` prelude.
19. Exit codes beyond 1. (A panic inside a task stops the program, decided per CC1.)

Performance and backend
20. Compile time: a cached pre-checked core (bigger win, more design) or trimming passes? Not asked yet.
21. After M4, keep the old generator as a fallback for unsupported functions, or require full MIR coverage?
22. Order for adopting the Goose-comparison items (sqrt intrinsics, bounds-check elimination, arena allocator, and so on), and the software `float.sqrt` loop in `compiler/core/core.pel`. See `docs/plans/bench-suite-findings.md`.

Minor
23. Confirm the `pool` and `link` names, only if he cares (NL2 was marked answered).
24. Dennis's earlier decisions not yet written into docs: a panic stops the program with no unwinding (add to `docs/design/errors.md` and `concurrency.md`).
