# Pelago

Pelago (from *pelagos*, Greek for "the open sea"; source files end in `.pel`) is a self-hosting systems language. It compiles to native code with its own backend, has no garbage collector and no runtime, and is memory safe through value semantics instead of a borrow checker. The syntax is braces and newlines, with a TypeScript-like feel.

```
//~ Sum the squares of 1 to n.
fn sum_squares(n: int) -> int {
    var total = 0
    for i of 1..=n {
        total += i * i
    }
    return total
}

fn main() {
    /// a plain comment
    print("sum of squares: #(sum_squares(10))")
}
```

Status: the stage 0 compiler, written in Rust with no dependencies, handles the whole language. It lexes, parses, resolves names, checks types and value semantics, lowers to an IR, optimises, and produces x86-64 executables for Linux (ELF) and Windows (PE) without a linker or libc. A formatter and a language server are built. The compiler is not yet rewritten in Pelago itself, and a few things are still open (real threads, `async` suspension, other CPUs); see [docs/NEXT.md](docs/NEXT.md) and the [task board](docs/plans/BOARD.md).

## The language in brief

- **Value semantics.** Values are read-only by default; `inout` and `sink` access modes are repeated at the call site, and `c.bump!()` changes a receiver while `c.close!!()` consumes it.
- **No exceptions, no null.** Errors are `result` values, absence is `option` (`none`), and `?` propagates.
- **One `switch`** for pattern matching, including list patterns. Exhaustiveness is checked.
- `fn`, `const` and `var`; lowercase type names; `obj` with inheritance written `:`; `~>` closures; `#name` and `#(expr)` inside strings; `meta` for compile-time code; `pub` and `priv`.
- **Symbol operators only.** No word operators.
- **Integer overflow traps.** Arithmetic that overflows stops the program (`stopped: arithmetic overflow`) instead of wrapping silently, and the native backend keeps that exact behaviour.

### Comments

Pelago has its own comment syntax (decided in [docs/design/comments.md](docs/design/comments.md)):

| Form | Meaning |
|---|---|
| `/// text` | line comment |
| `///* ... *///` | block comment (nests) |
| `//~ text` | doc comment for the item that follows |
| `//~* ... *~//` | doc block |

A plain `//` or `/* */` is a lexer error with a hint.

A guided tour of the syntax is [examples/tour.pel](examples/tour.pel) (it shows syntax, so it is not meant to type check as a program). [examples/programs/](examples/programs) holds real programs that check and run cleanly.

## Getting started

You need a Rust toolchain. From a checkout of this repository:

```
cargo build --release
cargo run -- run examples/hello.pel
cargo run -- build -o hello examples/hello.pel
```

The tool is called `pelago`; on Windows (PowerShell) use `cargo run -- ...` the same way, or put `target\release\pelago.exe` on your `PATH`.

### Commands

| Command | What it does |
|---|---|
| `pelago check <path>` | parse, resolve names and check types of a file or a directory |
| `pelago run <path>` | lower and run `main` in the IR interpreter (`--opt` optimises first, `--stats` counts, `--native` builds and runs machine code) |
| `pelago build [-o out] <path>` | build a native x86-64 executable, optimised by default (`--no-opt`, `--target=linux\|windows`, `--leak-check`) |
| `pelago ir <path>` | check, lower to the IR, verify and print it |
| `pelago fmt [--check] <path>...` | format `.pel` files in place; `-` filters standard input to standard output |
| `pelago lsp` | the language server, over stdio |
| `pelago highlight <file>` | colour a file (`--ansi`, `--html`, `--html-page`, `--plain`, `--theme dark\|light`) |
| `pelago lex`, `parse` | print tokens or the syntax tree and report errors (`lex-bench`, `parse-bench` time them) |
| `pelago keywords`, `tokens` | list the keywords and operators |
| `pelago grammar` | print the generated TextMate grammar |

### Formatter

`pelago fmt` is a token-based formatter: indentation, trailing spaces, blank lines, spacing around commas. It never touches comments, refuses files with errors, and checks that the result has the same tokens. `--check` exits non-zero if a file would change, which is what CI uses on `examples/`. The formatter is also served through the language server. It is configured by a `[fmt]` section in `pelago.toml`, read from the closest such file above the one being formatted:

```toml
[fmt]
indent = 4   # spaces per level, 1 to 8
width = 100  # 20 to 400; not used yet, it is for line wrapping
```

Unknown sections and keys are errors. Line wrapping from the syntax tree is next; see [S-07](docs/plans/cards/S-07-formatter.md).

### Language server

`pelago lsp` gives errors as you type, hover, go to definition, find references, rename, completion, signature help, document symbols, code actions, semantic tokens and formatting. It is checked by tests in `compiler/src/lsp/`; the design is in [S-06](docs/plans/cards/S-06-language-server.md).

## Editor support

| Editor | Repository | State |
|---|---|---|
| VS Code | [pelagolang/vscode-pelago](https://github.com/pelagolang/vscode-pelago) | highlighting, snippets and the language server; install from the [Visual Studio Marketplace](https://marketplace.visualstudio.com/items?itemName=pelagolang.pelago), or from the `.vsix` attached to each [release](https://github.com/pelagolang/vscode-pelago/releases) (not on Open VSX yet) |
| Zed | [pelagolang/zed-pelago](https://github.com/pelagolang/zed-pelago) | Tree-sitter highlighting plus the language server (`pelago` must be on your `PATH`); not published yet |
| Tree-sitter | [pelagolang/tree-sitter-pelago](https://github.com/pelagolang/tree-sitter-pelago) | grammar for Zed, Neovim and Helix |

The TextMate grammar in `editors/textmate/` is generated from the compiler's keyword table and kept in step by a test (`BLESS=1 cargo test editors` regenerates it); the VS Code repository receives a copy through a workflow.

## Speed

The native backend is held to a hard goal: every program in the benchmark suite at or below both C at `-O2` and Rust at `-O`, without giving up overflow traps. The suite is in [compiler/benches/](compiler/benches), each Pelago program next to a C and a Rust twin that print the same output.

```
sh compiler/benches/compare.sh            # needs gcc, python3 and optionally rustc; run from the repository root
sh compiler/benches/compare.sh --checked  # also compare against C -O2 -ftrapv and Rust with overflow checks on
sh compiler/benches/compare.sh --json --runs 7 collatz sieve
```

A snapshot (best of 3, one container, native `--opt`): geometric mean 0.97x of C `-O2` and 0.85x of Rust, at or below C `-O2` on 6 of 11 programs. Numbers move with every performance PR; the work plan is [docs/plans/a11-work-items.md](docs/plans/a11-work-items.md), and the backend is described in [docs/design/backend.md](docs/design/backend.md). CI also runs the suite on Windows.

## Repository layout

- `compiler/`: the stage 0 compiler (`src/`), the library (`core/core.pel`), the benchmarks (`benches/`) and the tests.
- `examples/`: `tour.pel`, `hello.pel` and `programs/`.
- `docs/design/`: design drafts for each part of the language and compiler (start with [basics](docs/design/basics.md), [types](docs/design/types.md), [memory](docs/design/memory.md), [checker](docs/design/checker.md), [ir](docs/design/ir.md)).
- `docs/plans/`: the task board and one card per piece of work.
- `editors/`: the generated TextMate grammar.
- [ROADMAP.md](ROADMAP.md): vision, decisions table and phases. [docs/NEXT.md](docs/NEXT.md): where we stopped. [NAMES.md](NAMES.md): how the name was chosen.

## Contributing

Run these before pushing:

```
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Branches are `<author>/<type>/<short-name>`, commit subjects and PR titles use [Conventional Commits](docs/contributing/commits.md), and every PR fills in the [template](.github/pull_request_template.md) ([worked examples](docs/contributing/pr-scenarios.md)). The compiler has no external dependencies on purpose; ask before adding one. [CLAUDE.md](CLAUDE.md) has more detail on conventions.

## Licence

MIT OR Apache-2.0, at your option: see `LICENSE-MIT` and `LICENSE-APACHE`.
