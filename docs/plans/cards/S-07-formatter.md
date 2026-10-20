Id: S-07
Title: `pelago fmt` and LSP formatting
Column: Doing (stages 1 and 2a built)
Order: -
Depends on: parser comment-keeping (step F1); S-06 for the editor side
Size: L, in stages
Revisit when: the owner has answered the questions below.

# Why

The formatter is on the roadmap ("Formatter, language server, editor syntax highlighting") and S-06 L6 lists formatting as left. The language server already runs; `textDocument/formatting` only needs a function from source text to source text. The roadmap also says the standard library and the formatter pick the naming style (`ROADMAP.md`, Naming row).

# Status

**Stage 1 built** (`compiler/src/fmt/`): a token-based layout pass. `pelago fmt [--check] <path>...` (files or directories; `-` filters stdin to stdout) and `textDocument/formatting` in the language server. It fixes indentation (4 spaces, one level for each open bracket line, plus the levels the author added for things brackets do not show, such as `case` bodies and `.method()` chains), trailing spaces, tabs, blank lines (one at most, none after `{` or before `}`), the space around commas, and one space before a trailing comment. Comments are tokens and are never touched. Safety: a file with errors is refused, the result is lexed again and must hold the same tokens, and a test checks idempotence and an unchanged tree on every example. All examples are already formatted.

**Why the first stage needs no parser change (F1):** the formatter lexes the file itself, so comments are already there; the parser does not have to keep them. F1 is dropped.

**Known limit of stage 1:** a line indented one level too far, on purpose or by mistake, is kept, because without the tree it cannot be told from a `case` body. Stage 2 derives indentation from the tree and fixes this; until then `fmt --check examples` in CI keeps our own files right, and `--check` reports only lines the layout would change.

**Configuration (owner: yes):** a `[fmt]` section in `pelago.toml`, found by walking up from the file; `indent` and `width` first. Built (`compiler/src/config.rs`): `indent` (1 to 8, default 4) and `width` (20 to 400, default 100; used by stage 2 wrapping). `pelago.toml` is the general config file for everything language-related, so other sections can come later; unknown sections and keys are errors with a line number. The CLI and the language server both read it.

```toml
[fmt]
indent = 2
width = 100
```

**Stage 2a built** (`fmt/wrap.rs`, `fmt/imports.rs`): a line longer than `width` is broken at the rightmost outermost `( )` or `[ ]` group that has a comma, one element per line with a trailing comma (question 5, 6); lines that are still long are broken again, element by element. Not broken: lines with comments or multi-line tokens, groups with a single element, groups with a top-level `;` or `of` (comprehensions, array types) or unbalanced `<` `>` (generic arguments), and `{ }` blocks. Names in `import { ... }` lists are sorted, `...` last (question 10); a list with a comment is left alone. The result must also parse and print the same tree as the input, and a test formats every example at widths 60, 40 and 20 and checks idempotence.

**Stage 2b (not built):** breaking long binary expressions (question 7), and deriving indentation from the tree so a line indented one level too far is corrected. `compiler/core/core.pel` is not formatted yet (it has a few long calls); CI checks `examples` only.

# What it needs

1. **F1, comments in the tree (dropped, see Status).** Today only `///` docs are kept (`docs` on items, fields, cases). Plain `//`, `/* */` and docs in other places are dropped. The lexer already emits all of them as tokens, so the cheapest route is to keep a side list of comment tokens (span, kind, and whether code comes before it on the line or a blank line comes before it) in `ParseOutput`, and let the printer re-attach them by position. The tree and its golden files do not change.
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
