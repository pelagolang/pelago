Id: B-02
Title: Choose the file extension
Column: Done (`.pel` chosen)
Order: 2 (track B)
Depends on: B-01
Size: S
Touches: `README.md`, `examples/`, `compiler/src/package.rs` (the `.pel` suffix check), docs, B-03

# What to decide

A short extension, free of clashes, that looks right in an editor tab and a file listing.

# Method

1. List 3 candidates from the name (initials, first syllables, the whole short name).
2. For each, check: the Markup language page and the common-extensions lists (document formats and other languages), GitHub linguist's list, and a plain web search "`.ext` file".
3. Criteria: 2 to 4 letters, lowercase, typed easily, not already a programming language, not a document or config format, not a Windows system type (`.sys`, `.dll`), not an executable type.
4. Prefer an extension that begins like the name, so the name and the file are connected.

# Known clashes to avoid

`.pl` Perl/Prolog, `.ml` OCaml, `.md` Markdown, `.rs` Rust, `.go`, `.zig`, `.nim`, `.tl` Teal, `.pc` pkg-config, `.pac` proxy config, `.atl` Eclipse ATL, `.th` various, `.lang` (our old placeholder; also used for localisation
files in games, which is another reason to replace it).

# Where the extension appears in this repository

- `compiler/src/package.rs` (finding source files by suffix), tests that read `*.pel`, `examples/*.pel`, `examples/programs/*.pel`, `compiler/core/core.pel`, `compiler/tests/examples.rs`, `.github/workflows/ci.yml` if it globs, `CLAUDE.md`, `README.md`, design docs that name files, and the editor files from B-03.

# Done means

- One extension chosen and recorded in `NAMES.md` and `ROADMAP.md`. Renaming is done in B-04 (mechanical search and replace, one commit).
