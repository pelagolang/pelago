Id: S-11
Title: Regex: linear lookahead and backreferences
Column: Ready (Build)
Order: -
Depends on: S-10 (the engine, `@regex`, done)
Size: L, in three steps
Revisit when: -

# Why

Two things from [docs/design/regex.md](../../design/regex.md) are planned but not built: lookahead that stays linear when its body can scan far (section R4), and backreferences (section 5, wanted by the owner on 2026-11-10).

# Plan

1. **Reversed-scan lookahead (built).** The Rust front end reverses the tree of a lookahead body (concatenations flipped, `^` and `$` swapped), a first pass over the text from the end gives one bit per position, the main loop reads the bit. The reference matcher does the same so the two stay comparable on the Python-checked corpus.
2. **Backreferences, Tier A (built).** A referenced group that can only match a finite set of strings (at most 256) is rewritten into plain alternatives; linear, no opt-in. Corpus cases checked against Python.
3. **Backreferences, Tier B (built; the budget is a parameter of the generated functions, no limit of 3 groups yet).** A memoising matcher over (program position, text position, spans of the referenced groups), polynomial for a fixed number of referenced groups, a mandatory deterministic budget and the result `matched | nomatch | gaveUp`. The compiler prints the tier and the group count and refuses more than 3 referenced groups unless the attribute raises it.

Also open, from S-10: the native-backend panic on `if (a || b) && state.caps[i * S] == pos` over an array in a struct (worked around in the generator; no small program found yet).
