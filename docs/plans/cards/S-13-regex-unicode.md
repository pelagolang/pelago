Id: S-13
Title: Regex: Unicode mode
Column: Done
Order: -
Depends on: S-11 (done)
Size: M
Revisit when: -

# Why

The engine works on bytes: `.` and classes see UTF-8 text one byte at a time, `\w` is ASCII, `(?i)` folds ASCII only. The owner answered on 2026-11-10 that Unicode mode is a follow-up card ([docs/design/regex.md](../../design/regex.md) section 4).

# Plan (built; what was built and what was left out is in `docs/design/regex.md` section 6)

Follow-up (same day, owner: "do it"): Unicode `\b`/`\B`, Python's extra case folds, `\p{..}` general categories, `(?u)` in the reference engine.

1. **Decide the surface** (propose, owner decides): a flag `(?u)` or the default for text patterns; what `.` means (one code point); which classes get Unicode tables (`\w`, `\d`, `\s`, general categories `\p{L}`).
2. **UTF-8 automata.** Compile a code point range into byte sequences (the standard UTF-8 range split), so the engine stays a byte machine and stays linear; no decoding step in the matcher.
3. **Case folding** for `(?iu)`: simple folding tables, generated at compile time into the class tables.
4. **Tests against Python** with non-ASCII text (Python's `re` is Unicode by default); the corpus generator gets a Unicode alphabet.
5. **Memory:** the tables for `\w` and `\p{L}` are large; classes stay in the constant tables of the program, and a pattern that does not use them pays nothing.
