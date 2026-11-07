# Smoke benchmark map

Which programs a PR runs, chosen by the paths it touches (see `docs/process/workflow.md`). Not wired to CI yet.

| PR touches | Programs |
|---|---|
| `compiler/src/lexer.rs`, `compiler/src/parser/` | lexer, wordfreq, kostya_json |
| `compiler/src/ir/`, `lower/`, `typecheck/` | collatz, fib38, nbody, matmul, sieve, structloop |
| `compiler/src/native/` | all of the above, plus hello (binary size), bintrees, levenshtein |
| heap, arena, runtime, `compiler/core/` | bintrees, strsplit, strbuild, mapwork, structlist |
| channels, tasks | add concurrency programs to `bench/programs/` first |
| anything else | collatz, lexer, bintrees |

Regression threshold: 5% or the measured spread, whichever is larger.
