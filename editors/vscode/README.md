# Pelago for VS Code

Syntax highlighting, bracket and comment settings, snippets, and two test colour themes for `.pel` files.

Install without any tool: copy this whole folder to `~/.vscode/extensions/pelagolang.pelago-0.0.1/` (on Windows
`%USERPROFILE%\.vscode\extensions\`), then restart VS Code. To package a `.vsix` instead, run `npx @vscode/vsce package` in this folder.

The grammar in `syntaxes/` is generated from the compiler's keyword tables: do not edit it by hand (see `../README.md`).
The two themes ("Pelago Dark (test)", "Pelago Light (test)") give every scope a visible colour, so you can check the grammar;
any normal theme works too.
