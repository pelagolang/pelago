# Roadmap

Working title: **lang** (see [NAMES.md](NAMES.md) for the name shortlist).

## Vision

A serious, self-hosting systems language:

- Compiled to native code by **our own backend**. No interpreter, no VM, no GC, no runtime.
- Static types with local inference. Readable like TypeScript, with the low-level feel of Rust.
- Memory safe through **value semantics**, not a borrow checker with lifetimes.
- Multi-paradigm from a small set of building blocks: procedural, functional, object-oriented (interfaces and composition), and generic/compile-time programming.
- Ends with a compiler written in the language itself.

## Decisions so far

| Area | Decision | Status |
|------|----------|--------|
| Execution | Ahead-of-time compiled, no GC, no runtime | decided |
| Typing | Static, with local type inference | decided |
| Syntax family | Braces, TS/Rust-like, no significant indentation | decided |
| Memory safety | Value semantics (read-only by default, plus `inout` / `sink` access modes; second-class references) | decided, pending the paper test below |
| Backend | One backend, our own, from day one | decided |
| Bootstrap | Stage 0 compiler in Rust, then rewritten in the language itself | decided |
| OO | Structs and interfaces, plus an `obj` kind with single inheritance (written with `:`) from the start (heap-allocated, singly owned, closed unless `open`). First feature to cut if compile speed or the safety model suffers | decided |
| Compile-time code | `meta` | decided |
| Modules | `mod` for modules, `pkg` reserved for bundles | decided |
| Functions | Optional named arguments; no overloading; `c.bump!()` marks calls that change or consume the receiver; shorthand types `T?` and `T ! E`; `main` in three forms | decided |
| Compile-time code | `meta`: expression, `meta fn`, `meta N: int`, `meta if` | decided |
| Tail calls | Explicit and guaranteed: `become f(x)` | decided |
| Async | Deferred; threads and channels first; `async` / `await` reserved | decided |
| Optimizer hints | Attributes: `@inline`, `@noinline`, `@cold` | decided |
| `switch` | One pattern-matching construct (`docs/design/switch.md`); `break` leaves the loop; bare names bind; `is` tests in conditions | draft |
| Types | `docs/design/types.md` decisions T1 to T9 recorded | decided |
| Memory layout | AoS by default; `soa<T>` opt-in collection with column access (`points.x`); compiler-chosen layouts later (`docs/design/memory.md`) | decided |
| Linear types | Opt-in `linear` modifier, staged after the type checker; `linear` reserved as a soft keyword | decided |
| Collections | `docs/design/collections.md` decisions C1 to C8 recorded (must-exist indexing, negative indexes, insertion-ordered maps, `string`/`str`, `+` and spread); builders (own comprehension shape: `[x of xs if x > 0 ~> x * 2]`) added | decided |
| Errors | `docs/design/errors.md` decisions E1 to E8: expected failures are `result`/`option`, bugs stop the program (no unwinding), overflow stops in every build with `+% -% *%` for wrapping, `converts` for `?` and `to` | decided |
| Maps | `map { "a": 1 }`, empty map `{:}` | decided |
| Closures | `x ~> x * 2` and `~> it * 2` | decided |
| Null | None. Absence is `none` in `option<T>` | decided |
| Ternary | Included, chains right, spaces around `?` and `:`; error-propagation `?` is attached | decided |
| `++` / `--` | Included as statements only; `+= 1` stays | decided |
| Loops | `for x of items` (values), `for k in map` (keys, ranges), `for i, x of items` | decided |
| Operators | Symbols only (`&&` `\|\|` `!` `^` `%`), no word forms | decided |
| Access modes | `let` / `inout` / `sink` apply everywhere a name is bound | decided |
| Visibility | `pub` and `priv`; private is the default | decided |
| Functional | Closures, enums with pattern matching, higher-order functions | decided |
| Functions | `fn` (chosen because `fnc` splits into two tokens in most tokenizers) | decided |
| Variables | `var` (changeable) and `const` (truly immutable) | decided |
| Types | Lowercase names; sized ints `int8` to `int64`, `uint8` to `uint64` | decided |
| Naming | The compiler accepts any identifier style; the standard library and formatter pick one | decided |
| `int` | Always exactly 64 bits; sized types for dense data | decided |
| Conversions | Written `x to float`, allowed only when nothing can be lost; lossy ones get a different spelling | decided in principle |
| Generics | `<T>` | decided |
| Statements | Newlines end statements, no semicolons | decided |
| Call sites | `inout` repeated at the call site; `copy` explicit for owning types | decided |
| Kernel / OS use | Parked for now | parked |
| Name | Undecided | open |

## Targets (measured, not felt)

| Goal | How we measure it | Target |
|------|-------------------|--------|
| Compile speed | Same project in equivalent Rust / C / C++, wall-clock | Faster than `rustc` and `g++`; chase `tcc`/Go-class speeds |
| Runtime speed | A fixed benchmark set against Rust and C | Parity first, then a few percent ahead via no-alias optimization |
| Token efficiency | The same programs through real LLM tokenizers | Fewer tokens than Rust and TS for equivalent programs |
| Safety | Programs that would be use-after-free or data races elsewhere | Rejected at compile time |

Compile speed and token counts are tracked from the first compiler, in CI.

---

## Phase 0: Design on paper (current phase)

No compiler code yet. Everything here is cheap to change.

- [ ] **Sample programs.** Write 8 to 10 programs in the imagined syntax. Draft in [docs/design/samples.md](docs/design/samples.md)
- [ ] **Settle the open syntax questions** listed at the bottom of that file
- [ ] **Informal grammar.** Extract keywords, operators and constructs from the samples into a short EBNF
- [ ] **Semantics notes.** One short page each, in `docs/design/`:
  - [ ] Value semantics: copy vs move, read-only / `inout` / `sink`, what a reference may and may not do
  - [ ] Closures: what they capture and how they are owned
  - [ ] Generics and interfaces: static dispatch by default, dynamic dispatch opt-in
  - [ ] Error handling (`Result` plus `?`, or an alternative)
  - [ ] Compile-time evaluation (the `comptime` idea from Zig)
  - [ ] Concurrency: out of scope for v0, but write down the intended direction
- [ ] **Paper test.** Sketch the compiler's own core data (tokens, syntax tree, symbol table, type checker state) under value semantics. If something does not fit, revisit the safety model before writing any compiler code
- [ ] **Token benchmark.** Run the samples and equivalent Rust and TS programs through real tokenizers. Record the numbers
- [ ] **First target CPU** for the backend (x86-64 is the usual start) and first OS / object format (ELF on Linux)
- [ ] Pick the final name, then rename the repo
- [ ] Choose a license

**Milestone:** the samples read well, the paper test passes, and the grammar fits on a couple of pages.

---

## Phase 1: Stage 0 front end (Rust)

- [x] Cargo workspace and CI (build, test, format, lint)
- [ ] Benchmark harness for compile time
- [x] Keyword table, token kinds and spans (`compiler/src/{keyword,token,span}.rs`)
- [ ] Source files and a diagnostics system (clear errors with line, column and a source snippet)
- [x] **Lexer** (`compiler/src/lexer.rs`): about 214 MB/s, 59 M tokens/s in a release build on the first benchmark (`langc lex-bench`)
- [ ] **Parser** (recursive descent with Pratt parsing for expressions) producing the AST
- [ ] AST printer and a golden-file test suite (`.lang` input, expected output)
- [ ] Parse every sample program

**Milestone:** all samples parse, with good errors for bad input.

## Phase 2: Meaning

- [ ] Name resolution and modules
- [ ] **Type checker** with local inference, structs, enums, generics and interfaces
- [ ] **Value-semantics checker:** exclusivity of `inout` access, moves vs copies, `sink`
- [ ] Pattern-match exhaustiveness
- [ ] Generics via monomorphization
- [ ] A first IR (SSA-based) and an IR printer

**Milestone:** programs are type-checked and lowered to IR, and the checker rejects the unsafe examples.

## Phase 3: Own backend

- [ ] x86-64 instruction selection
- [ ] Register allocation (linear scan first, because it is fast to compile)
- [ ] Emit object files / ELF directly
- [ ] Decide the linking story: system linker first, our own later
- [ ] Minimal startup code and system-call layer, no libc dependency if possible
- [ ] Hello world and FizzBuzz run as native binaries

**Milestone:** the language produces real executables with no third-party backend.

## Phase 4: A usable core library

- [ ] Allocators, arrays, strings, hash maps, with no GC
- [ ] File and console I/O
- [ ] Compile-time evaluation
- [ ] Enough language and library to write a compiler

## Phase 5: Self-hosting

- [ ] Rewrite the compiler in the language (stage 1), compiled by the Rust stage 0
- [ ] Compile the compiler with itself (stage 2), then again (stage 3)
- [ ] Stage 2 and stage 3 produce identical output, so we are self-hosting
- [ ] Keep a pinned stage-0 seed so the chain can always be rebuilt

## Phase 6: Beat the baselines

- [ ] Optimizer that uses no-alias facts from value semantics (inlining, loop optimizations, vectorization)
- [ ] Benchmarks against Rust and C, published and tracked
- [ ] More targets: AArch64, then RISC-V
- [ ] Compile-time profiling and speedups

## Phase 7: Tooling and community

- [ ] Formatter, language server, editor syntax highlighting
- [ ] Package manager and build tool
- [ ] Language reference, tutorial, website and playground
- [ ] CONTRIBUTING.md, issue templates, first tagged release

---

## Parked ideas

- Using the language for an operating system kernel (freestanding mode)
- Machine-learning-guided optimization heuristics
- Automatic cache-friendly data layouts
- More CPU targets (PowerPC, MIPS)

## Recommended reading

- *Crafting Interpreters*, Robert Nystrom (free online). Covers lexing, parsing and semantics even though it builds interpreters.
- *Engineering a Compiler*, Cooper and Torczon, for IR, optimization and backends.
- The Hylo language documents, the closest design to our safety model.
- Zig's `comptime` documentation, for compile-time evaluation.
