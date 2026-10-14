# The lexer

Source: `compiler/src/lexer.rs`. It turns source text into tokens and never fails: bad input
becomes an error token plus an entry in the error list, and lexing carries on.

## What it does today

- Hard keywords become keyword tokens; soft keywords (`from`, `to`, `step`, ...) are plain identifiers
  that the parser interprets.
- Newlines are tokens (a run of blank lines is one). Comments are tokens too, so a formatter can keep them:
  `//`, nested `/* */`, `///`, and `///* ... *///`.
- Numbers: decimal with underscores, `0x`, `0b`, `0o`, floats and exponents. After a `.`, digits are a plain
  integer, so `t.0.1` is a tuple access and not a float.
- Characters and strings with escapes. A string is one token, including `#name` and `#(expr)` parts, and the
  lexer finds the real end of the string even with nested strings inside `#(...)`.
- Operators use longest match, so `>>=` is one token.

Speed on the first benchmark (10 MB, release build, `pelago lex-bench`): about 214 MB/s, 59 million tokens a second.

## Is the lexer final?

No, and it shouldn't be. Here's what will change it:

- **Interpolation splitting.** Strings are one token now. A later pass must split `"a #(x + 1) b"` into text and expression parts.
- **Contextual rules from the parser,** such as splitting `>>` when it closes generics.
- **Settled details:** numeric suffixes (`255u8`), raw and multi-line string rules, whether `=>` is ever needed (the `switch` design says no).
- **Safety:** protection against look-alike Unicode characters in names.
- **Speed work:** interning names so the compiler compares numbers, not strings.
- **Editor use:** re-lexing only the changed part of a file, for a language server.

It's solid as a base, and the tests will catch regressions as it grows.
