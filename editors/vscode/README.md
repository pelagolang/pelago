# Pelago for VS Code

Syntax highlighting, bracket and comment settings and snippets for `.pel` files.

Install without any tool: copy this whole folder to `~/.vscode/extensions/pelagolang.pelago-0.0.1/` (on Windows
`%USERPROFILE%\.vscode\extensions\`), then restart VS Code. To get a `.vsix`, run `editors/build-vsix.sh` (needs only `zip`); it writes `editors/dist/pelago-<version>.vsix`. In VS Code: Extensions view, `...` menu, "Install from VSIX...".

The grammar in `syntaxes/` is generated from the compiler's keyword tables: do not edit it by hand (see `../README.md`).
The grammar in `syntaxes/` names the *kind* of each word with scopes that all end in `.pelago`. The extension ships **no colour theme** and
never touches your theme or workbench colours. To make `.pel` files look different from Rust or TypeScript under any theme, `package.json`
contributes `configurationDefaults` for `editor.tokenColorCustomizations`: every rule is anchored to `source.pelago`, so it applies to `.pel`
files only. The colours are mid-tones (about 3.5:1 or better on both white and `#1e1e1e`) so they read on dark and light themes. To change
them, copy a rule into your own `editor.tokenColorCustomizations` in settings; yours win.
