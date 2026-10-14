Id: S-05
Title: Tree-sitter grammar and the Zed extension
Column: Shelf
Order: -
Depends on: nothing in the compiler (the syntax is settled); the owner's approval of the tools below
Size: M for the grammar, S for the extension
Revisit when: the owner wants Zed (or Neovim, Helix) support, or before the first public release

# Parked on purpose (owner decision)

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
