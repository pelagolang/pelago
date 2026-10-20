<!--
Fill in what applies, delete what does not. Keep it short: a reviewer should know in 30 seconds what changed and why.
Worked examples for each kind of change: docs/contributing/pr-scenarios.md
Branch name: <author>/<type>/<short-name>, e.g. claude/perf/const-division, dennis/feat/for-range
-->

## Summary

**Before:** <what happened, or was missing, before this change>

**After:** <what happens now>

<One sentence on what the change does, if it is not obvious from the above.>

## Type

<!-- Keep one. It should match the branch type. -->

- [ ] `feat`: new language feature, command or capability
- [ ] `fix` / `bug`: something was wrong, now it is right
- [ ] `perf`: faster code, a faster compiler, or a smaller output
- [ ] `refactor`: no change in behaviour
- [ ] `docs`: docs, plans, comments only
- [ ] `chore`: CI, tooling, dependencies of the build, formatting

## Why

<The problem or the card behind it. Link the plan card (`docs/plans/cards/...`), issue or earlier PR.>

## How

<The approach in a short paragraph. Name the files or stages touched (`lexer`, `parser`, `typecheck`, `lower`, `ir`, `native`, `lsp`, ...) and any design choice a reviewer should check.>

## Evidence

<!-- Tests prove behaviour, numbers prove speed. Fill the block for your type; delete the rest. -->

### Checks (every PR that touches code)

```
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```

- [ ] All three pass locally
- [ ] New or changed behaviour has a test (name it): <test names>
- [ ] Linux and Windows CI are green (the Windows job runs the native programs for real)

### Performance (`perf`, or anything that can change generated code)

Run `compiler/benches/compare.sh --runs 7` from the repository root. Goal: every row at or below **both** C `-O2` and Rust `-O`.

| program | before (s) | after (s) | C -O2 (s) | Rust -O (s) | vs -O2 | vs Rust |
|---|---|---|---|---|---|---|
| <name> | | | | | | |

- Geometric mean vs C -O2: <before> -> <after>
- Rows at or below C -O2: <n of m> -> <n of m>
- [ ] Output of every benchmark still matches its C twin (no `OUTPUT DIFFERS`)
- [ ] No row got slower than before, or the regression is explained below
- [ ] Machine and OS used: <cpu, os, gcc and rustc versions>

### Compiler or language behaviour (`feat`, `fix`)

- Example that shows it (a `.pel` snippet, the command, and its output):

```
<paste>
```

- [ ] `cargo run -- check examples` is clean; `examples/tour.pel` updated if syntax changed
- [ ] `BLESS=1 cargo test` run and the golden `.ast` diff reviewed (syntax changes only)

## Language and docs in step

<!-- Delete if nothing here applies. -->

- [ ] Decision recorded in the design doc under `docs/design/` and in the `ROADMAP.md` decisions table
- [ ] `docs/design/keywords.md` and `compiler/src/keyword.rs` agree; editors regenerated (`BLESS=1 cargo test editors`)
- [ ] New syntax: parser, `ast_print.rs`, `visit.rs`, `check.rs`, `examples/tour.pel` and a parser test all done
- [ ] Plan card and `docs/plans/BOARD.md` moved to the right column
- [ ] No dropped word came back (`class`, `match`, `when`, `extern`, `comptime`, `inline`, `null`, word operators)
- [ ] No new external dependency (or the owner agreed to it first)

## Risks and follow-ups

<What could break, what you did not do on purpose, and what comes next. Write "none" if none.>

## Reviewer notes

<Where to start reading, what to run, what you are unsure about.>
