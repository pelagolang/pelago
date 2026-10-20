Id: S-07
Title: `pelago fmt` and LSP formatting
Column: Backlog (design only; waiting for the owner's answers to the style questions)
Order: -
Depends on: parser comment-keeping (step F1); S-06 for the editor side
Size: L, in stages
Revisit when: the owner has answered the questions below.

# Why

The formatter is on the roadmap ("Formatter, language server, editor syntax highlighting") and S-06 L6 lists formatting as left. The language server already runs; `textDocument/formatting` only needs a function from source text to source text. The roadmap also says the standard library and the formatter pick the naming style (`ROADMAP.md`, Naming row).

# What it needs

1. **F1, comments in the tree.** Today only `///` docs are kept (`docs` on items, fields, cases). Plain `//`, `/* */` and docs in other places are dropped. The lexer already emits all of them as tokens, so the cheapest route is to keep a side list of comment tokens (span, kind, and whether code comes before it on the line or a blank line comes before it) in `ParseOutput`, and let the printer re-attach them by position. The tree and its golden files do not change.
2. **F2, a source printer** (`fmt.rs`, next to `ast_print.rs`, which prints the debug tree and is not reusable). One function per node, a line-width-aware layout (try one line, else break), comments emitted when the printer passes their position.
3. **F3, `pelago fmt <path>`** with `--check` (exit 1 if a file would change, for CI) and `-` for stdin; **F4, `textDocument/formatting`** and range formatting (whole statements only) in the language server.
4. **Safety net:** formatting is idempotent, never changes the tree (parse, format, parse, compare trees ignoring spans), never loses a comment (count them), and runs over every file in `examples/` in a test. A file with syntax errors is left untouched.

# Open questions (each with my recommendation)

1. **Line width.** Recommend **100**. Every file in `examples/` already fits, and Rust (100) is the neighbour people read next to this. Alternatives: 80 (more wrapping), 120.
2. **Indent.** Recommend **4 spaces**, no tabs, as in all the examples. Not configurable.
3. **Configuration.** Recommend **none at first**: one style, like `gofmt`. A `pelago.toml` `[fmt]` section can come later if someone needs it.
4. **Braces.** Recommend the opening brace on the same line, as in `tour.pel`. One-line bodies stay on one line when they fit (`fn clear(inout xs: [int]) { xs.clear!() }`); a block with more than one statement always breaks.
5. **Trailing commas.** Recommend: when a list breaks across lines, one element per line **with** a trailing comma (smaller diffs); when it fits on one line, none.
6. **Breaking long calls and signatures.** Recommend: first try the whole thing on one line; else put each argument or parameter on its own line, indented once, closing bracket on its own line. No "fill" packing.
7. **Long binary expressions.** Recommend breaking **after** the operator at the lowest-precedence operator that makes it fit, continuation indented once. (Breaking before the operator would clash with line-end newlines being tokens: check with the parser before choosing; this is the one point where I need to look at newline handling.)
8. **Blank lines.** Recommend: keep the author's blank lines but collapse runs to one; none right after `{` or before `}`; exactly one blank line between top-level declarations is **not** forced (we do not rewrite spacing the author chose), except that imports stay together at the top.
9. **Comments.** Recommend: comments are never reflowed or re-wrapped, keep their text byte for byte (trailing spaces trimmed). A trailing `// x` stays on its line, one space after the code (aligned to nothing). A comment alone on a line takes the indent of the next statement.
10. **Import lists.** Recommend: sort names inside `{ a, b: c, ... }` alphabetically, keep `...` last; do **not** reorder or merge import statements (that is a lint, not formatting).
11. **Naming style** (the roadmap row says the formatter picks it). Recommend: the formatter does **not** rename anything. Naming is the linter's job, and renaming needs the resolver. We can decide the house style for the library separately.
12. **`use` vs `import`.** Both spellings exist. Recommend the formatter leaves the spelling alone.
13. **Name and flags.** `pelago fmt` (matches `check`, `run`, `build`); `--check`; stdin with `-`. OK?

# Plan once answered

F1 (comment side list, own PR, small) first, since it is independent of the style answers; then F2 with the decided style plus the safety-net tests; then F3 and F4. Each is its own PR.
