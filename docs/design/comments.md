# Comments: a syntax of our own (draft, waiting for the owner)

## Where we are

Today (`lexer.rs`, `keywords.md`, `lexer.md`): `// line`, `/* block */` (nests), `/// doc line`, `///* doc block *///`, and `//// ...` as an ordinary comment (so a banner of slashes is not a doc). The owner's view: `//` and `/* */` already exist in C, Java, Rust and Go, so they are not our own; `///` should be the ordinary line comment and `///* ... *///` the multi-line one. Drop `////`.

## What a comment syntax has to cover

1. A note to the reader, to the end of the line.
2. A note over several lines, or in the middle of a line (`f(1, /* why */ 2)`).
3. Commented-out code (a region, quickly on and off).
4. Documentation of the item that follows (shown in hover, used by tools).
5. Documentation of the file or module itself (a header; nothing follows it).
6. Markers for tools (`TODO`, formatter off/on, a `#!` first line for scripts).

Two practical tests for any choice:

- **Toggle test.** Editors comment a selection by adding one fixed line prefix and removing it again. The prefix must not be the start of another comment form, or toggling a doc line strips half of it (`/// doc` loses `//` and becomes `/ doc`). This is the bug the VS Code extension has today.
- **Cost test.** Every form is a lexer rule, a grammar rule (TextMate, tree-sitter), an editor config entry and a line in the formatter. Fewer forms, less to keep in step.

## Options

**A. The owner's spelling, two forms.** `/// text` is the ordinary comment, to the end of the line. `///* ... *///` is the multi-line comment, and it is also the doc comment (one line or many). No `//`, no `/* */`, no `////`.
- Covers 1 (line), 2 (block), 3 (many `///` lines, toggles cleanly), 4 and 5 (a `///*` block before an item, or at the top of the file).
- Does not give a separate doc line or an inline mid-line comment unless the block is used for it, and then a block comment is a doc comment: `f(1, ///* why */// 2)` would count as a doc that belongs to nothing.
- Toggle test: line prefix `///` is the start of `///*`, so toggling a doc block line has the same half-strip problem, though doc blocks are rare to toggle.

**B. The owner's spelling, four forms, kept apart.** `/// text` ordinary line; `///* ... *///` ordinary block (nests); `//! text` doc line; `//!* ... *!//` doc block. A doc form never begins with the ordinary line prefix, so the toggle test passes. `//` alone is no comment (an error that says "use `///`").
- Covers 1 to 6 (`//!` at the top of a file documents the file).
- Four forms; one more rule to learn than C-family languages, but each one is a plain variation on the one before.

**C. A different family, using `#`.** `# text` ordinary line, `#* ... *#` block, `## text` doc line, `#** ... **#` doc block. `#` is free outside strings today (it only means interpolation inside them).
- Short, familiar from scripting languages, and nothing in common with C.
- A `#!` first line fits naturally. Cost: `##` starts with `#`, so the toggle test fails for doc lines; interpolation `#name` in strings reads differently from a comment `#`.

## Recommendation

**B.** It keeps your `///` and `///*` exactly as the ordinary forms, fixes the toggle problem by design, separates documentation from notes (so a commented-out region is never a doc), and covers every case in the list. A is the smallest, but it makes documentation and block comments the same thing, which gives the inline case no home.

## Questions

1. A, B or C?
2. Does `///* ... *///` nest (comment out a region that already holds a block)? Recommendation: yes, as `/* */` does today.
3. Is a lone `//` an error (with a hint to write `///`) or ignored? Recommendation: an error. A leftover `//` from habit would otherwise silently be code.
4. Doc blocks: in B, `//!* ... *!//` is the doc block. Acceptable spelling, or prefer something else (`///**`)?
5. Do doc comments attach to the item that follows only, with `//!` for the enclosing module or file? Recommendation: yes.
6. Should a banner like `/////////////` be allowed as an ordinary comment? In B it already is (a `///` line); no extra rule.
7. Do we keep reading `//` and `/* */` for one release, with a `pelago fmt` rewrite, or switch at once? Recommendation: switch at once; there are no users yet, only our examples.

## What changing it touches

`lexer.rs` (the `slash` rule, plus `#` if C), `token.rs`, the highlighter, `keywords.md`, `lexer.md`, `basics.md`, `parser.md` (doc comments), every `.pel` file in `examples/`, `tree-sitter-pelago`, the TextMate grammar and the VS Code and Zed language configs. A one-off rewrite of the examples is easy to script.
