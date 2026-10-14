Id: B-03
Title: Syntax highlighter (TextMate grammar, VS Code extension, `pelago highlight`)
Column: Ready (no blockers; the extension is `.pel`)
Order: 3 (track B)
Depends on: nothing. B-01 and B-02 only change names, not the work.
Size: M
Touches: a new top-level folder `editors/`, `pelago` (one subcommand), CI, docs

# 1. Why

Plain white text makes the language hard to read and hard to like. Colour is also the cheapest way to *test the syntax design*: if something looks confusing when coloured,
it is probably confusing to read. The compiler already has everything needed: the lexer knows every token kind, hard keyword, soft keyword and operator.

# 2. Three layers, built in this order

| Layer | What | Who benefits | Cost |
|---|---|---|---|
| **1. TextMate grammar** | One JSON file describing tokens by pattern. | VS Code, many other editors (Sublime, Zed, JetBrains with a plugin, GitHub's renderers after registration). | Small |
| **2. VS Code extension** | Wraps the grammar, adds comment/bracket settings, snippets, a colour theme for testing. Installed locally from a `.vsix`. | The owner, immediately | Small |
| **3. `pelago highlight`** | Our own lexer prints the file in colour: ANSI for the terminal, HTML for docs and the website. | Terminal use, documentation, screenshots, and as the reference for correctness | Small |

Later (not in this card): semantic highlighting through a language server (colour a name by what it *is*: a type, a parameter, a mutable variable) and a Tree-sitter grammar for Neovim, Helix and Zed.
Those become worthwhile once an LSP exists.

# 3. What gets which colour (scope plan)

Names use the standard TextMate scope vocabulary so existing themes colour them well.

| Token / construct | Scope |
|---|---|
| Hard keywords of control flow: `if else unless switch case guard for in of while do loop break continue return defer become yield is` | `keyword.control.<name>` |
| Declaration keywords: `fn struct enum interface impl obj mod pkg import export use pub priv meta macro deinit let` | `keyword.declaration.<name>` (`storage.type` for `fn`, `struct`, `enum`, `interface`, `impl`, `obj`) |
| Binding keywords: `var const` | `storage.modifier` |
| Access modes: `inout sink copy` | `storage.modifier.access` (distinct, they are the language's signature) |
| `true false none` | `constant.language` |
| `self Self super` | `variable.language` |
| `unsafe async await` | `keyword.other` |
| **Soft keywords** (`type ext union static operator init new delete open abstract override final any default step to from it where linear test asm volatile atomic scope spawn chan select`) | `keyword.other.soft`, **only in the position where they act** (see section 4); elsewhere they are ordinary names |
| Types: lowercase names after `:` `->` `<` `,` in type position, and the built-in type names (`int float bool char string str list map set option result box ptr void never ...`) | `entity.name.type` / `support.type` |
| Function names at declaration and at call (`name(`) | `entity.name.function` / `support.function` |
| Call-site marks: `!` and `!!` after a method name, `inout x`, `sink x` | `keyword.operator.mark` |
| `~>` | `keyword.operator.arrow` (it is the closure arrow and the builder arrow) |
| `?` `??` `?.` `?:` | `keyword.operator.optional` |
| Other operators | `keyword.operator` |
| Numbers `42 1_000 0xFF 0b1010 0o17 3.14 1e-3` | `constant.numeric.*` |
| Characters `'a'` | `string.quoted.single` (escapes `constant.character.escape`) |
| Strings `"..."` | `string.quoted.double`; escapes `\n \t \\ \" \u{...}` as `constant.character.escape` |
| **Interpolation** `#name` and `#(expr)` inside strings | `punctuation.definition.interpolation` for `#` and the parens, `meta.interpolation` for the region (embedded language: the whole expression grammar again, so keywords and numbers inside `#( ... )` are coloured) |
| Comments `// ...`, `/* ... */` (nest), doc comments `/// ...`, `///* ... *///` | `comment.line`, `comment.block`, `comment.line.documentation`, `comment.block.documentation` |
| Attributes `@derive(...)`, `@inline`, `@must_use` | `meta.attribute` with `entity.name.function.attribute` |
| Generic parameters in `<...>`, labels `name:` in calls, field names | `variable.parameter`, `variable.other.member` |

# 4. The hard parts, and how to handle each

1. **Soft keywords.** They are identifiers except in one grammatical position. A regex cannot know the position. Approach: in the TextMate grammar, colour a soft keyword only when a *position pattern* matches: `to` / `to?` between an expression and a type (after an identifier, `)` or literal and before a type name), `step` after a range, `from` after `import`, `it` inside a closure body (heuristic: only when the file has `~>` earlier in the line), `init` / `deinit` at the start of a member, `scope` / `select` / `spawn` followed by `{` or `(`, `where` after a signature, `any` before an interface name in type position. Accept a small number of false negatives rather than colouring ordinary names wrongly. `pelago highlight` (layer 3) is exact because it uses the real lexer plus the parser's position knowledge, so it is the reference.
2. **String interpolation with nesting.** `"a #(f("x#y")) b"`: use TextMate `begin`/`end` rules with a recursive `include` of the expression patterns inside `#( ... )`, and a rule for `#identifier` (identifier characters only, so `#name.field` colours only `name`, as the lexer does).
3. **Nested block comments.** TextMate cannot count depth, but it can nest rules: a `begin: /\*` `end: \*/` rule that includes itself.
4. **Newline-sensitive syntax** (statements end at newlines): the grammar does not need to understand statements; it only colours tokens.
5. **Generic angle brackets versus comparison** (`a < b` vs `list<int>`): colour `<` after a type name (lowercase identifier immediately followed by `<` and then a type-looking token with no space) as punctuation; otherwise an operator. Heuristic, and fine.
6. **Keeping the keyword list in sync.** The grammar must contain exactly the keywords in `keyword.rs`. Do not copy by hand: generate (section 5).

# 5. Layout and generation

```
editors/
  README.md                         how to install and test
  textmate/
    pelago.tmLanguage.json            GENERATED from a template + keyword lists (do not edit by hand)
    template.tmLanguage.json        the hand-written part with markers like @HARD_KEYWORDS@
    tests/                          snippets with expected scopes (see section 6)
  vscode/
    package.json                    contributes: language, grammar, snippets
    language-configuration.json     comments, brackets, auto-closing pairs, indentation rules, word pattern
    snippets/pelago.json              fn, struct, enum, switch, for, closure, derive
    themes/                         a test theme so every scope has a visible colour
    README.md
```

Generation: add `pelago keywords --json` (hard list, soft list, punctuation list, type names). A small `pelago editors` command, or a test in `compiler/tests/`, fills the template and writes
`pelago.tmLanguage.json`. A **test fails if the committed file differs from the generated one**, which is how it stays in sync with `keyword.rs` automatically (CLAUDE.md rule: "keywords change in both places").

# 6. `pelago highlight`

```
pelago highlight <file> [--ansi | --html | --html-page] [--theme dark|light]
```

- Uses the real lexer. For positional soft keywords it asks the **parser** (the syntax tree has the positions of `to`, `step`, `init`, ... ) so output is exact. Implementation: run the lexer for all tokens, run the parser, collect the spans of soft keywords that the parser actually *used as keywords*, then colour those.
- Interpolation: reuse the string-splitting the parser already does (`StrPart::Interp`) so nested expressions are coloured with the same rules.
- Colours: one shared theme table (`kind -> style`), used for ANSI (256-colour with a plain fallback) and HTML (`<span class="t-kw">`, plus a small CSS block and a `--html-page` wrapper with the CSS inline, light and dark through `prefers-color-scheme`).
- `NO_COLOR` is respected. `-` reads standard input.
- Use for the documentation: write a docs build step later that colours the code blocks in `docs/design/*.md` (or use the TextMate grammar through a Markdown renderer).
- No dependencies (project rule): the highlighter is plain Rust over the existing lexer.

# 7. Tests

1. **Token coverage test** (Rust): lex every `examples/*.pel` and `compiler/core/core.pel`, run `highlight` in plain mode, and assert that the output with colour codes stripped is identical to the input (no byte lost or duplicated).
2. **Keyword sync test**: the generated grammar contains every hard and soft keyword and every punctuation entry; committed file equals regenerated file.
3. **Scope tests** for the TextMate grammar: a folder of small `.pel` files each with an expected list of `(text, scope)` pairs. Run them with a tiny tester. The standard tool is `vscode-tmgrammar-test` (npm), which would be a *development-only* dependency of the `editors/` folder and not of the Rust project. **Ask the owner before adding any npm tool** (project rule: no dependencies without asking); the alternative is a minimal Rust test that loads the JSON and runs the regexes itself, which is more work but dependency-free.
4. **Interpolation tests**: `"a #b c"`, `"#(x + 1)"`, `"#(f("nested"))"`, `"##"`, an unterminated string at end of line.
5. **Soft keyword tests**: `it` as a name outside closures, `to` as a field name, `step` in a range, `init` as a method name.
6. **Golden HTML**: highlight `examples/tour.pel` to HTML and compare with a committed file (refresh with an env var, like `BLESS=1` for the parser's golden trees).

# 8. Steps

1. `pelago keywords --json`; template + generator + sync test.
2. TextMate grammar: comments, strings (with escapes and interpolation), numbers, hard keywords, operators; scope tests. Look at `examples/tour.pel` in a real editor and fix what looks wrong.
3. Types, function names, attributes, soft-keyword positions.
4. VS Code extension: package, language configuration, snippets, test theme. Instructions to install: `code --install-extension` from the packaged `.vsix` (packaging with `vsce` is an npm tool, so give the manual way too: copy the folder into `~/.vscode/extensions/`).
5. `pelago highlight --ansi`, then `--html`; tests 1 and 6.
6. Docs: `editors/README.md`, a line in `README.md` and `CLAUDE.md` commands.

# 9. Later, noted so it is not forgotten

- GitHub syntax highlighting for the repository's `.pel` files needs a registration in GitHub Linguist (a pull request with the grammar and a usage threshold of many public repositories). Not available until the language is public and used.
- Tree-sitter grammar (`tree-sitter-<name>`) for Neovim, Helix and Zed. A decent chunk of work; the parser doc `docs/design/parser.md` is the source.
- Semantic tokens from a language server.
- A formatter (`pelago fmt`) will want the same trivia handling; the comment tokens are already kept by the lexer for that.

# Done means

- Opening `examples/tour.pel` in VS Code with the extension shows keywords, access modes, types, strings with interpolation, numbers and comments in distinct colours.
- `pelago highlight --ansi examples/tour.pel` and `--html` produce the same colouring and lose no characters.
- The sync test fails if someone edits `keyword.rs` without regenerating the grammar.
- `editors/README.md` explains installing and testing; fmt, clippy and tests are green.
