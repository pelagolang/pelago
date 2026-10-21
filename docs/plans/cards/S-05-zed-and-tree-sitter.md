Id: S-05
Title: Tree-sitter grammar and the Zed extension
Column: Done (owner)
Order: -
Depends on: nothing in the compiler (the syntax is settled); the owner's approval of the tools below
Size: M for the grammar, S for the extension
Revisit when: the owner wants Zed (or Neovim, Helix) support, or before the first public release

# Status

Built on the branch `claude/programming-language-design-psjufd-3` of both repositories: the grammar (`grammar.js`, an external scanner in C, queries, 71 corpus tests; all examples
and `core.pel` parse with no error nodes; commit `05899d4`) and the Zed extension (`extension.toml` pinning that commit, the language config, the queries, a Rust crate that starts
`pelago lsp`; it builds for `wasm32-wasip1`). **Not done:** trying it inside Zed itself (the owner has since tried it; see the checklist below), publishing to the Zed extensions registry,
and a licence for the repositories (the owner's choice). Known: the generated `parser.c` is about 17 MB (9,300 states); the compiled parser is a few MB. Differences from the
compiler's parser are listed in the grammar's README (`break name` is always a label, a touching `<` is generic only if `>` and `(`, `{` or `.` follow on the line, the
grammar is more forgiving than the compiler and does not run `check.rs`).

# Check and publishing preparation (2026-10-20)

Done from the cloud sandbox (Linux), in `tree-sitter-pelago`, `zed-pelago`, `vscode-pelago` and this repository:

- Grammar: `tree-sitter test` 71 of 71, `script/check-corpus.sh` 19 of 19 files, `tree-sitter build --wasm` gives a 2.5 MB parser, all four query files compile. CI now builds the wasm too and uses `npm ci`.
- Zed extension: builds for `wasm32-wasip1` and `wasm32-wasip2` (Zed's docs now name `wasip2`), `cargo fmt` and `clippy -D warnings` clean, queries identical to the pinned grammar commit. CI (`ci.yml`, `script/check-queries.sh`) checks all of it and builds the pinned grammar to wasm.
- Language server: `pelago lsp` driven over stdio the way Zed starts it (initialize, didOpen, diagnostics, shutdown): it answers, advertises hover, definition, references, completion, symbols and semantic tokens, and reports `expected an expression, found `let`` for a bad line.
- VS Code: the grammar tokenized all 19 `.pel` files (18,800 tokens) with the real engine (`vscode-textmate` and Oniguruma), no failures; `vsce package` builds a valid `.vsix` (icon, banner, repository, keywords, changelog). Found and fixed three highlighting bugs in the generated TextMate grammar: function and type names after `fn` / `obj` / `struct` were never scoped as declarations (the keyword rule won), and the `step` and `it` soft-keyword rules coloured the text before the word as a keyword.
- Publishing material: `PUBLISHING.md` in `zed-pelago` (registry PR contents) and in `vscode-pelago` (Marketplace and Open VSX steps), an icon (placeholder), licence notes. Neither extension ships or changes a theme (guard: `scripts/check.js`).

Not doable in the sandbox, for the owner to check by hand on Windows (PowerShell):

1. Zed: the owner already tried the extension in Zed (2026-10-20), so no hands-on check is listed here. The registry asks for a test at the exact commit submitted; reinstall the dev extension once after the final pin.
2. VS Code: `npm ci` then `npm run package` in `vscode-pelago`, install `dist/pelago-<version>.vsix` (Extensions, `...`, Install from VSIX). Open a `.pel` file: language mode shows `Pelago`, `fn`/`obj` names, `step`, `it`, strings and interpolation are coloured; switch between a light and a dark theme (colours must stay readable, and `.rs` / `.ts` files must look exactly as before); snippets appear when typing `fn` and `for`.
3. Decisions: the licence is chosen (`MIT OR Apache-2.0`, files in every repository); still open: making the repositories public (they are private; the registry needs the grammar and extension public), the logo (`vscode-pelago/images/icon.svg` is a placeholder), the first public version number, and creating the publisher accounts (Marketplace publisher `pelagolang`, Open VSX namespace `pelagolang`).
4. Then publish, in this order: make the repositories public, pin the final `tree-sitter-pelago` commit in `zed-pelago` (`script/check-queries.sh`), open the registry PR from `PUBLISHING.md`, `vsce publish` and `ovsx publish`. Nothing is published or submitted yet.

# Original shelving note (kept for the history)

The owner created two empty repositories under the `pelagolang` organisation and chose to shelve this work until the backend (A-09) is under way.

- `pelagolang/tree-sitter-pelago`: the Tree-sitter grammar (`grammar.js`, `queries/highlights.scm`, `queries/brackets.scm`, `queries/indents.scm`, `queries/outline.scm`, test corpus in `test/corpus/`).
- `pelagolang/zed-pelago`: the Zed extension (`extension.toml`, a `languages/pelago/` folder with `config.toml` and copies of the queries; later a small Rust crate that starts the language server).

# Facts to remember

- Zed does **not** read TextMate grammars. It needs a Tree-sitter grammar. Our TextMate grammar and the VS Code extension (`editors/`) do not carry over, but `pelago keywords --json` gives the word lists to put in `grammar.js` and the queries.
- The grammar must follow `docs/design/parser.md`. The oracle is the compiler's own parser: every file in `examples/` and `examples/programs/` must parse without error nodes.
- Soft keywords (`from`, `to`, `step`, `it`, `type`, `init`) are plain identifiers except where they act as keywords; the grammar needs the same positional rules as `highlight.rs`.
- Interpolation inside strings (`#name`, `#(expr)`) needs an external scanner or a careful string rule.
- Tools: the Tree-sitter CLI (needs Node; Node 22 is installed in the cloud container) for the grammar. The extension crate needs `zed_extension_api` and a `wasm32-wasip1` build. Both are outside `compiler/`, which keeps its no-dependency rule. **Awaiting the owner's approval of both** (asked twice, not answered yet).
- A published Zed extension is added by a pull request to the `zed-industries/extensions` repository, which points at our repository and a commit.
- Check the extension id and display name for clashes before publishing (project rule for new names).

# Plan when it comes off the shelf

1. Write `grammar.js` from `parser.md`; test corpus from the examples; `tree-sitter test` green; `tree-sitter parse` on every example without errors.
2. Queries: highlights (reuse the class names of `highlight.rs`), brackets, indents, outline, injections for strings if needed.
3. `zed-pelago`: `extension.toml`, language config (`.pel`, line comment `//`, brackets), the queries, a README.
4. A script in `pelago` that checks the grammar's keyword list against `keyword.rs` (as `editors.rs` does for TextMate).
5. Language server hookup comes with S-06.
