# Pelago for VS Code

Syntax highlighting, bracket and comment settings, snippets, and two colour themes for `.pel` files.

Install without any tool: copy this whole folder to `~/.vscode/extensions/pelagolang.pelago-0.0.1/` (on Windows
`%USERPROFILE%\.vscode\extensions\`), then restart VS Code. To get a `.vsix`, run `editors/build-vsix.sh` (needs only `zip`); it writes `editors/dist/pelago-<version>.vsix`. In VS Code: Extensions view, `...` menu, "Install from VSIX...".

The grammar in `syntaxes/` is generated from the compiler's keyword tables: do not edit it by hand (see `../README.md`).
The grammar only names the *kind* of each word (keyword, type, string...); the colours come from the theme you have chosen, so under a
stock theme the file looks like TypeScript or Rust. Choose "Pelago Dark" or "Pelago Light" (Command Palette: "Preferences: Color Theme")
for Pelago's own palette.
