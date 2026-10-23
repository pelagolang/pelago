# Pull request scenarios

Worked examples for `.github/pull_request_template.md`. Copy the one closest to your change, then replace the details.
Every example keeps the same order: **Summary** (before and after), **Why**, **How**, **Evidence**, **Risks**.

All numbers, test names and cards below are made up to show the shape. Use your real output.

Common rules:

- Branch: `<author>/<type>/<short-name>`, type is one of `feat`, `fix`, `bug`, `perf`, `refactor`, `docs`, `chore`.
  Claude's branches are `claude/<type>/<short-name>`.
- PR title and every commit subject: [Conventional Commits](commits.md), `type(scope): summary`, imperative, lowercase start, under 70 characters, no trailing period. Example: `perf(native): divide by constants with a multiply`. The types are `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `chore`, `ci`, `build`; add `!` for a breaking change.
- Delete template sections that do not apply; do not leave them empty.
- Numbers come from the tool, pasted as printed. Never round a loss into a win.

## 1. Performance change (the main case)

Branch `claude/perf/const-division`, title `perf(native): divide by constants with a multiply`.

> **Before:** `divmod` emitted `idiv` for every `/` and `%`, 0.412 s against C `-O2` at 0.301 s.
>
> **After:** division by a constant becomes a multiply and shift, as gcc does. `divmod` runs in 0.288 s.
>
> ## Why
> Card A-11, work item "division by constants". `divmod` was the slowest row (1.37x vs `-O2`).
>
> ## How
> `ir/opt` rewrites `x / c` and `x % c` for `c > 0` into the magic-number form; `native/` needs no change.
> Signed and unsigned are separate paths; `i64::MIN / -1` is left alone.
>
> ## Evidence
> `bench/compare.sh --runs 7`, x86-64 Linux, gcc 13.2, rustc 1.84:
>
> | program | before (s) | after (s) | C -O2 (s) | Rust -O (s) | vs -O2 | vs Rust |
> |---|---|---|---|---|---|---|
> | divmod | 0.412 | 0.288 | 0.301 | 0.299 | 0.96x | 0.96x |
> | collatz | 0.530 | 0.529 | 0.540 | 0.538 | 0.98x | 0.98x |
> | sieve | 0.210 | 0.210 | 0.215 | 0.214 | 0.98x | 0.98x |
>
> - Geometric mean vs C -O2: 1.04x -> 0.99x
> - Rows at or below C -O2: 6 of 9 -> 7 of 9
> - Every output matches its C twin. Nothing got slower.
> - Tests: `div_by_constant_matches_idiv` (all small constants, edge values), `--opt` interpreter run agrees.
>
> ## Risks
> Wrong magic numbers give silently wrong results, so the test sweeps every divisor from 1 to 1000 against `idiv`.

Tips for perf PRs:

- Show only the rows that moved plus the worst row still above 1.00x, so the next target is visible.
- If a row got slower, say so in the table and explain it under Risks. A loss that is not mentioned will be found anyway.
- Run on a quiet machine and keep `--runs 7` or more. A win under 1% needs `--runs 15` before it counts.
- A benchmark change (new program, new twin) is its own PR so the numbers stay comparable.

## 2. Bug fix

Branch `claude/fix/shadowed-loop-var`, title `fix(lower): give a shadowed loop variable its own slot`.

> **Before:** `for i in 0..3 { let i = i * 2; ... }` printed `0 0 0` (the inner `i` overwrote the loop counter slot).
>
> **After:** it prints `0 2 4`.
>
> ## Why
> Found while writing `examples/programs/...`. The lowerer reused the slot of the outer `i`.
>
> ## How
> `lower/` allocates a fresh slot for each `let`, even when the name is already bound in an outer scope.
>
> ## Evidence
> Failing test first: `lower::shadowed_loop_var` failed on `main`, passes here. `cargo run -- run repro.pel` prints `0 2 4`.
> fmt, clippy, tests green on Linux and Windows.
>
> ## Risks
> One extra slot per shadowing `let`; the optimiser's slot merging removes it (see `benches` unchanged: geomean 0.99x before and after).

Tips: always include a test that fails without the fix, and the smallest program that reproduced it.

## 3. Language or semantics change

Branch `dennis/feat/for-step`, title `feat(parser): add step to for ranges`.

> **Before:** `for i in 0..10` always counted by one.
>
> **After:** `for i in 0..10 step 2` counts by two. `step` stays a soft keyword.
>
> ## Why
> Decision 14 in `ROADMAP.md`; plan card A-12.
>
> ## How
> Parser reads the optional `step` expression, checker requires an integer, lowering emits an add of the step.
>
> ## Evidence
> - Parser test `for_step`, checker test `step_must_be_integer`, run test `for_step_counts`.
> - `BLESS=1 cargo test` rewrote `tour.ast`; diff reviewed, one line added.
> - `cargo run -- check examples` clean.
>
> ## Language and docs in step
> - [x] `docs/design/basics.md` and the `ROADMAP.md` decisions table updated in this PR
> - [x] `parser`, `ast_print.rs`, `visit.rs`, `check.rs`, `examples/tour.pel` all done
> - [x] `keywords.md` and `keyword.rs` unchanged (`step` was already soft)
>
> ## Risks
> A loop with `step 0` would never end; the checker rejects a literal zero and the run-time check traps otherwise.

Tips: a semantics change records the decision and the reason in the docs in the same PR (see `CLAUDE.md`). Keep the performance table
if the change touches generated code; otherwise say "no codegen change".

## 4. Docs-only change

Branch `claude/docs/pr-template`, title `docs: add a pull request template and scenarios`.

> **Before:** PRs had free-form descriptions.
>
> **After:** every PR starts from the same template, with worked examples in `docs/contributing/pr-scenarios.md`.
>
> ## Why
> Reviewers asked for evidence (tests, benchmark numbers) in the same place every time.
>
> ## How
> Two new files, no code. Type: `docs`.
>
> ## Evidence
> Links resolve; no code touched, so fmt, clippy and tests are unaffected (CI still runs).

Tips: skip the Checks and Performance blocks. Say "docs only" so nobody waits on a benchmark.

## 5. Refactor or chore (no behaviour change)

Branch `claude/refactor/split-lowering`, title `refactor(lower): split expr.rs into smaller modules`.

> **Before:** `lower/expr.rs` had 3,400 lines.
>
> **After:** four modules, same behaviour.
>
> ## Evidence
> Tests unchanged and green. `compare.sh` generated code is byte-identical: `pelago ir` output diffed over `examples/programs` with no changes.
> Compile time (`cargo run --release -- parse-bench`) 1.02x -> 1.02x.
>
> ## Risks
> None expected; moves only.

Tips: "no behaviour change" is a claim, so show the proof: identical IR output, identical benchmark outputs, or unchanged tests.

## 6. Editor, LSP or other-repo change

Branch `claude/feat/hover-types`, title `feat(lsp): show types on hover`.

> **Before:** hovering a name in Zed showed nothing.
>
> **After:** it shows the inferred type.
>
> ## Evidence
> `cargo test lsp`, plus a screenshot or transcript of the request and response. If `tree-sitter-pelago` or `zed-pelago` needs a matching change,
> link that PR and say which merges first. Zed diagnostics on `examples/` stay clean.
>
> ## Risks
> The extension needs the new `pelago` on PATH (on Windows: `cargo install --path . --force`, then restart Zed).

## Checklist before you press "Create"

1. Title and branch follow the rules above.
2. Before and After are written for someone who has not seen the code.
3. The Evidence block matches the type and carries real output.
4. Sections that do not apply are deleted.
5. CI is running; fix red before asking for review.
