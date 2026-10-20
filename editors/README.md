# Editor support

Three layers, all generated from or checked against the compiler's own tables:

| Layer | Where | Use |
|---|---|---|
| `pelago highlight` | the compiler | Colours a file in the terminal (`--ansi`), as HTML (`--html`, `--html-page`) or not at all (`--plain`). Exact: it uses the real lexer. `--theme dark|light`. `NO_COLOR` is respected. |
| TextMate grammar | `textmate/` | The grammar editors read. `template.tmLanguage.json` is written by hand; `pelago.tmLanguage.json` is **generated** from it and from `keyword.rs` / `token.rs`. |
| `pelago lsp` | the compiler | Language server: diagnostics, hover, go to definition, references, outline, completion, semantic tokens. |
| VS Code extension | `vscode/` | Wraps the grammar, adds language settings, snippets and Pelago-only colour rules (no theme); `build-vsix.sh` packs it into `dist/pelago-<version>.vsix`. See `vscode/README.md`. |

## Keeping the grammar in step

A test (`compiler/src/editors.rs`) fails when `textmate/pelago.tmLanguage.json` differs from what the template and the keyword tables
produce. After changing `keyword.rs`, `token.rs` or the template, run

```
BLESS=1 cargo test editors
```

which rewrites both generated copies (`textmate/pelago.tmLanguage.json` and `vscode/syntaxes/pelago.tmLanguage.json`).
`pelago grammar` prints the generated grammar; `pelago keywords --json` prints the word lists.

## Checking the colours

```
pelago highlight examples/tour.pel               # in the terminal
pelago highlight --html-page examples/tour.pel > tour.html
```

Soft keywords (`to`, `step`, `it`, `init`, ...) are coloured only where they act as keywords; elsewhere they are ordinary names.
`pelago highlight` decides this from the tokens around the word. The TextMate grammar uses patterns for the same positions, so it can
miss a few cases; the highlighter is the reference.

## Not done yet

Built, waiting for a try-out in Zed: a Tree-sitter grammar and the Zed extension (`docs/plans/cards/S-05-zed-and-tree-sitter.md`; repositories `pelagolang/tree-sitter-pelago` and `pelagolang/zed-pelago`).
Built: `pelago lsp`, the language server (`S-06`). Any editor that can start a command and speak LSP over stdio can use it; VS Code needs a small client extension (not written: it would need the `vscode-languageclient` npm package).

GitHub colouring (needs a Linguist registration once the language is public), a Tree-sitter grammar for Neovim, Helix and Zed, and
semantic colouring from a language server.
