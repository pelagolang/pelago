<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/brand/logo/pelago-horizontal-dark.svg">
    <img alt="Pelago" src="assets/brand/logo/pelago-horizontal.svg" width="360">
  </picture>
</p>

<p align="center"><strong>A systems language that compiles straight to machine code.</strong><br>No garbage collector. No runtime. No linker. Integer overflow stops the program.</p>

<p align="center">
  <a href="https://pelagolang.github.io/website/">Website</a> ·
  <a href="https://pelagolang.github.io/website/en-US/docs/">Documentation</a> ·
  <a href="https://pelagolang.github.io/website/en-US/docs/getting-started/install/">Install</a> ·
  <a href="https://pelagolang.github.io/website/en-US/docs/tour/">Tour</a> ·
  <a href="https://pelagolang.github.io/website/benchmarks/">Benchmarks</a>
</p>

# Pelago

Pelago (from *pelagos*, Greek for "the open sea"; source files end in `.pel`) is a compiled systems language with braces and newlines for syntax and a small set of strict rules for memory and errors:

- **Value semantics instead of a borrow checker.** Values are read-only by default. Mutation and consumption are named at the call site: `c.bump!()` changes a receiver (`inout`), `c.close!!()` consumes it (`sink`).
- **No exceptions and no null.** Failure is a `result` (`int ! parse_error`), absence is an `option` (`none`), and `?` passes a failure up.
- **Exact integer overflow.** Arithmetic that overflows stops the program with `stopped: arithmetic overflow` and a source location. It never wraps silently, and the native backends keep that behaviour.
- **One `switch`** for pattern matching, including list patterns, with exhaustiveness checking.
- **Its own backend.** `pelago build` writes the executable file itself (ELF, PE or Mach-O), without a C compiler, a linker or libc. On Linux a hello world is about 4 KB.

The compiler in this repository is the stage 0 compiler. It is written in Rust with no dependencies. Pelago is not yet rewritten in Pelago, and the project is pre-release: there are no binary releases yet and the language can still change.

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
    print("sum of squares: #(sum_squares(10))")
}
```

Errors are values, and a *braid* (a Pelago data type) holds a fixed number of slots whose types alternate:

```
/// Errors are values: `! parse_error` says the function can fail, `?` passes the failure on.
fn parse_all(items: [str]) -> [int] ! parse_error {
    var out: [int] = []
    for item of items {
        out.push!(item.parseInt()?)
    }
    ok(out)
}

fn main() {
    switch parse_all(["1", "2", "x"]) {
        case ok(values): print("sum: #(values.iter().sum())")
        case err(e): print("bad number: #(e.message)")
    }

    /// A braid alternates its slot types: here int, string, int, string.
    const b: (int, string; 4) = (1, "one", 2, "two")
    print(b[0], b[1], b[2], b[3])

    /// Overflow stops the program. It never wraps.
    var x: int8 = 120
    x += 10
    print(x)
}
```

```
bad number: bad digit at position 0
1 one 2 two
stopped: arithmetic overflow
```

More programs are in [`examples/programs/`](examples/programs) (they type check and run), a syntax sampler is [`examples/tour.pel`](examples/tour.pel), and a small multi-file project with CI is in [pelagolang/examples](https://github.com/pelagolang/examples).

## Install

There are no prebuilt downloads yet. Until the first release, build `pelago` from source with a [Rust toolchain](https://rustup.rs); the steps for Windows (PowerShell), Linux and macOS are on the [install page](https://pelagolang.github.io/website/en-US/docs/getting-started/install/), and release channels will be announced there too.

From a checkout, `cargo build --release` puts the tool at `target/release/pelago` (`pelago.exe` on Windows).

## Quick start

```
pelago run hello.pel                # build machine code in memory and run `main`
pelago run --interp hello.pel       # the IR interpreter instead (--opt optimises first, --stats counts steps)
pelago build -o hello hello.pel     # write a native executable, optimised by default
pelago check src                    # parse, resolve names and check types of a file or a directory
pelago fmt --check .                # check formatting; drop --check to format in place
```

`pelago run` and `pelago build` take a file or a directory, and both take `--no-opt`, `--leak-check` (report memory that was never freed) and `--timings` (milliseconds per phase). The [run and build guide](https://pelagolang.github.io/website/en-US/docs/getting-started/run-and-build/) has the details.

| Command | What it does |
|---|---|
| `pelago run <path>` | build and run `main`; `--interp` uses the IR interpreter, with `--opt` and `--stats` |
| `pelago build [-o out] <path>` | native executable; `--target=linux\|windows\|macos`, `--arch=x86_64\|aarch64` (macOS is AArch64 only), `--symbols` keeps symbols for gdb and objdump |
| `pelago check <path>` | names and types, no code generation |
| `pelago ir <path>` | lower to the IR, verify it and print it |
| `pelago fmt [--check] <path>...` | the formatter; `-` filters standard input; configured by `pelago.yaml` |
| `pelago lsp` | the language server, over stdio |
| `pelago highlight <file>` | colour a file in the terminal or as HTML |
| `pelago std [--json]` | list the standard library modules by group |
| `pelago lex`, `parse`, `keywords`, `tokens`, `grammar` | tools for working on the language itself |

## Targets

`pelago build` cross-compiles: pick the system with `--target` and the CPU with `--arch`.

| | x86-64 | AArch64 |
|---|---|---|
| Linux | yes | yes |
| Windows | yes | yes |
| macOS | no | yes |

The output is a static executable that talks to the operating system directly (system calls on Linux and macOS, `kernel32` on Windows). `pelago run` runs a program only on the machine it was built for. CI builds and runs the native test programs on Linux (x86-64 and ARM64), Windows (x86-64 and ARM64) and macOS (Apple silicon). The AArch64 backend is the youngest and sees the least testing; macOS support covers the basics so far (clock, arguments, files, sleep).

## What is in the box

Pelago ships with a standard library written in Pelago. The full list, grouped, is `pelago std`; the site documents each module in the [library reference](https://pelagolang.github.io/website/en-US/docs/library/).

- **Core types.** Integers from `int8` to `int256` and unsigned twins, `byte`, `float16` to `float256`, `str` and `string` with UTF-8 text, lists, maps, sets, options, results, iterators, boxes and arenas.
- **Braids.** Fixed-size sequences of alternating types, `(int, string; 3)`, with pair and strand views. See the [braids guide](https://pelagolang.github.io/website/en-US/docs/guide/braids/). This feature is experimental.
- **Concurrency.** `spawn` returns a task that you `join`; tasks run on their own small stacks, and Linux x86-64 uses real operating system threads with futexes and atomics. AArch64 and the other systems are catching up.
- **Text and data.** `std.json` (with `@derive(fromJson)` for your own types) and `std.regex` (an own engine, with Unicode classes).
- **Files and system.** `std.io`, `std.fs`, `std.path`, `std.os`, `std.env`.
- **Time.** `std.time`, `std.calendar`, `std.zone` with a bundled tz database, and `std.timefmt`.
- **Random numbers.** `std.random` (seeded generators) and `std.csprng` (secure).
- **Hashes and checksums.** SHA-2, SHA-3, BLAKE2, BLAKE3, RIPEMD, SM3, fast hashes, checksums and the legacy ones.
- **Cryptography.** MACs, key derivation, password hashing (including bcrypt), ChaCha and AEAD ciphers, Poly1305. Pelago's own implementations are checked against test vectors, but they have not been audited, so treat them as a work in progress.
- **Big numbers.** `std.big` for integers beyond the machine's width.

### Comments

| Form | Meaning |
|---|---|
| `/// text` | line comment |
| `///* ... *///` | block comment (nests) |
| `//~ text` | doc comment for the item that follows |
| `//~* ... *~//` | doc block |

A plain `//` or `/* */` is a lexer error with a hint.

## Editors and tools

`pelago lsp` is a language server: errors as you type, hover, go to definition, find references, rename, completion, signature help, outline, code actions, semantic tokens and formatting.

| Editor | Repository | |
|---|---|---|
| VS Code | [pelagolang/vscode-pelago](https://github.com/pelagolang/vscode-pelago) | highlighting, snippets and the language server |
| Zed | [pelagolang/zed-pelago](https://github.com/pelagolang/zed-pelago) | highlighting and the language server (`pelago` must be on your `PATH`) |
| Neovim, Helix and others | [pelagolang/tree-sitter-pelago](https://github.com/pelagolang/tree-sitter-pelago) | the Tree-sitter grammar |

Setup steps for each are on the [editors page](https://pelagolang.github.io/website/en-US/docs/getting-started/editors/). The TextMate grammar in `editors/textmate/` is generated from the compiler's keyword table and kept in step by a test.

## Packages: Islet

[Islet](https://github.com/pelagolang/islet) is the package manager for Pelago. It is early: packages are source that the compiler reads next to yours, stored once by hash in a global store, with signing from the start. Manifests (`islet.yaml`, `islet.lock`) are YAML. A public registry does not exist yet.

## Speed

The target is every program of the benchmark suite at or below C at `-O2` and Rust at `-O`, without giving up overflow traps. The suite is in [`bench/`](bench): each Pelago program sits next to C, C++ and Rust versions that print the same output. Numbers move with every performance change, so the current ones are on the [benchmarks page](https://pelagolang.github.io/website/benchmarks/).

```
python3 bench/harness/run.py [--runs N] [--size small,medium,large] [name ...]   # needs python 3.11+, gcc; optionally g++, clang, tcc, rustc
```

## Building and testing the compiler

```
cargo build --release
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```

The code is in `compiler/src/` (lexer, parser, name resolution, type checker, IR and its optimiser, the native backends, formatter, language server). The standard library is in `compiler/core/` and `compiler/std/`, each with a `tests/` directory of `.pel` programs and their expected output.

## Contributing

Run the three checks above before pushing. Branches are `<author>/<type>/<short-name>`, commit subjects and PR titles use [Conventional Commits](docs/contributing/commits.md), and every PR fills in the [template](.github/pull_request_template.md) ([worked examples](docs/contributing/pr-scenarios.md)). The compiler has no external dependencies on purpose; ask before adding one. See [SECURITY.md](https://github.com/pelagolang/.github/blob/main/SECURITY.md) to report a vulnerability.

## Licence

MIT OR Apache-2.0, at your option: see [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE).
