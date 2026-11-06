# Handoff: bump arena allocator (card A-13)

Repo: `pelagolang/pelago`. Read `CLAUDE.md`, then `docs/design/arena.md` and `docs/plans/cards/A-13-arena.md`.

## Goal
An owned `arena` with bump allocation, checked `aref<T>` handles and a one-store `reset!`, built as a library over `rt_alloc`/`rt_realloc`/`rt_free` (no OS calls, so the no-libc runtime period is unaffected). Target row: `bintrees`.

## Merged (all CI green)
- #135 `docs(arena)`: `docs/design/arena.md`, card A-13, ROADMAP row (marked "proposed, awaiting owner"), pointer in `memory.md`.
- #141 `feat(core)`: `compiler/core/arena.pel` (`arena()`, `new!`, `get`, `set!`, `reset!`, `used`, `aref<T>`), intrinsic `rt_needs_drop<T>()` (`lower/intr.rs`, `core/rt.pel`), 5 tests in `compiler/src/lower/tests.rs` (`arena_*`: interpreter, optimiser, native, leak check).
- #149 `perf(bench)`: `bench/programs/bintrees/pelago_arena.pel`. Same output (1310700). Native, medium size, 3 quick runs in a cloud container: box 0.10 to 0.15 s, arena 0.066 to 0.079 s (about 1.6x). Not the harness.

## Open PRs / unpushed work
None. Everything is on `main`.

## Design in one paragraph
Handle = 8 bytes: low 32 bits byte offset, high 31 bits epoch. Offsets (not pointers) survive growth by `rt_realloc` and moving the arena. `get` returns a bit-for-bit copy and stops on a stale epoch ("used a handle after the arena was reset"). `reset!` bumps a global epoch counter. v1 holds only types without drop glue; others stop at run time ("an arena holds only types without drop glue"). 4 GiB limit ("arena full"). Limits: the epoch counter wraps after 2^31 bumps and is not atomic (needs an atomic once real threads land, card A-12 C6).

## Next steps, in order
1. Ask Dennis the five questions at the end of `docs/design/arena.md` (plain types only in v1 and drop lists later; mark/release; handle width; names; ambient arena for `box`). `docs/design/heap.md` (PR #143, from the heap research) recommends headerless region allocators and arenas on reserved ranges; reconcile it with arena.md after his answers.
2. AR4: inline `new`/`get` fast paths in the optimiser (keep `top`/`cap`/`base` in registers in loops). Measure with one `bintrees` run.
3. Make "type has drop glue" a compile-time error instead of a run-time stop (checker follow-up).
4. AR5: lazy `str.chars()` for the `lexer` row. Not a library change: `iter<T>` is an eager `list<T>` (`compiler/core/iters.pel`). Smallest option: a lowering rule that decodes UTF-8 in place for `for c of s.chars()`. Needs its own card.
5. AR6 (optional): drop lists so non-plain types work (reverse-order drop at `reset!`/`deinit`). Only if Dennis says yes.
6. Other rows from the plan: `mapwork` needs map storage (card S-01), `strsplit` allocates little. An arena does not fix these.

## Build and test (PowerShell, from the repo root)
```
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test arena            # the 5 arena tests (fast)
cargo test                  # full suite, about 90 s
cargo run --release -- build -o bt_arena.exe bench/programs/bintrees/pelago_arena.pel
.\bt_arena.exe              # prints 1310700
python3 bench/harness/run.py --runs 7 bintrees   # only when asked; Dennis's usage was tight
```

## Project rules to keep
- Branch `claude/<type>/<name>` style: `<author>/<type>/<short-name>` (never random suffixes). PR titles and commits: Conventional Commits. Use `.github/pull_request_template.md` with every box ticked (N/A marked). Merge your own PR once CI is green; docs-only PRs may merge without waiting.
- Comment syntax in `.pel` is `///` (plain `//` is an error). `a.new!(...)`, `a.set!(...)`, `a.reset!()` need the `!` because the receiver is `inout`.
- Do not run full benchmark suites unless asked. Do not clone upstream benchmark repos.
- No libc ever; a panic stops the program (no unwinding).
