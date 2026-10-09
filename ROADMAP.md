# Roadmap

Working title: **lang** (until we pick a name — see [NAMES.md](NAMES.md)).

This is a checklist of objectives, grouped by phase. Phase 0 is mostly
**questions to answer** — the answers shape everything after it, so do them first.
Tick boxes as decisions get made or work gets done, and record decisions in
`docs/decisions/` (one short file per decision).

---

## Phase 0 — Vision & core decisions

### Purpose
- [ ] **Why does this language exist?** Write a one-paragraph pitch.
- [ ] **Who is it for?** (beginners, systems programmers, data/scripting, game dev, web, education, yourself)
- [ ] **What problem does it solve better than existing languages?** Name 2–3 languages it competes with or borrows from.
- [ ] **Is this a learning project or something meant for real users?** (affects how much polish/tooling matters)
- [ ] Define 3–5 **design principles** to settle future arguments (e.g. "readability over brevity", "no hidden allocations").

### Execution model
- [ ] **Interpreted, compiled, or both?**
  - Tree-walking interpreter (simplest, slowest)
  - Bytecode VM (good middle ground)
  - Compile to native (via LLVM, Cranelift, QBE, or C)
  - Transpile to another language (JS, C, Go…)
  - Target WebAssembly
- [ ] **What language is the implementation written in?** (Rust, Go, C, Zig, OCaml, Python, TypeScript…)
- [ ] Long term: should it become **self-hosting** (compiler written in itself)?

### Type system
- [ ] **Static or dynamic typing?** (or gradual)
- [ ] **Type inference?** None / local / full (Hindley–Milner style)
- [ ] **Null handling:** nulls allowed, or `Option`/`Maybe` types?
- [ ] **Generics?** If yes, how (monomorphization, erasure)?
- [ ] **Algebraic data types / pattern matching?**
- [ ] Interfaces, traits, typeclasses, or classes with inheritance?

### Paradigm & semantics
- [ ] **Primary paradigm:** imperative, functional, object-oriented, multi-paradigm?
- [ ] **Mutability default:** immutable by default or mutable by default?
- [ ] **Error handling:** exceptions, result types, error values (Go-style), or effects?
- [ ] **Memory management:** garbage collection, reference counting, ownership/borrowing, manual, or arenas?
- [ ] **Concurrency model:** threads, async/await, green threads/coroutines, actors, channels — or none at first?
- [ ] Expressions vs statements: is everything an expression?

### Syntax & feel
- [ ] **Syntax family:** C-like braces, Python-like indentation, Lisp-like, ML-like, something new?
- [ ] Semicolons: required, optional, or none?
- [ ] Write 5 **example programs** in the imagined syntax: hello world, FizzBuzz, a struct/record, error handling, a small generic function.
- [ ] Decide naming conventions and keyword list (keep it small).
- [ ] Source file extension (e.g. `.lang` for now).

---

## Phase 1 — Minimal working language

- [ ] Set up repo structure, build system, and CI (build + tests on every push)
- [ ] Choose a license (MIT / Apache-2.0 / dual)
- [ ] Write an informal **grammar** (EBNF) for a tiny subset
- [ ] **Lexer** — turn source text into tokens (with line/column info)
- [ ] **Parser** — tokens → AST (recursive descent or Pratt parsing recommended)
- [ ] AST pretty-printer (debugging aid)
- [ ] **Evaluator / interpreter** for: integers, booleans, strings, arithmetic, variables, `if`, loops, functions
- [ ] **REPL**
- [ ] Run a file from the command line: `lang run hello.lang`
- [ ] Clear **error messages** with line/column and source snippet
- [ ] Test suite: golden-file tests (`.lang` input → expected output)

**Milestone:** FizzBuzz and recursive Fibonacci run.

---

## Phase 2 — A real language

- [ ] Semantic analysis pass (name resolution, scoping)
- [ ] **Type checker** (if static)
- [ ] Data structures: arrays/lists, maps, structs/records
- [ ] Closures and first-class functions
- [ ] Enums / sum types and pattern matching (if chosen)
- [ ] Error handling mechanism from Phase 0
- [ ] Modules and imports
- [ ] Minimal **standard library**: printing, strings, math, collections, file I/O
- [ ] Switch to bytecode VM or native backend (if planned)

**Milestone:** write a small non-trivial program (e.g. a JSON parser, a to-do CLI, or a text adventure) in the language.

---

## Phase 3 — Tooling & ecosystem

- [ ] Formatter (`lang fmt`)
- [ ] Language Server (LSP) for editor support: diagnostics, go-to-definition, completion
- [ ] Syntax highlighting (TextMate grammar / tree-sitter) for VS Code etc.
- [ ] Package manager / build tool
- [ ] Debugger support or at least good stack traces
- [ ] Benchmarks and performance tracking
- [ ] Fuzzing the lexer/parser

---

## Phase 4 — Docs & community

- [ ] Pick the final **name** and rename the repo
- [ ] Language reference / spec
- [ ] Tutorial ("Learn X in Y minutes" style)
- [ ] Website + online playground (WASM build makes this easy)
- [ ] Logo
- [ ] CONTRIBUTING.md, code of conduct, issue templates
- [ ] Versioning policy and first tagged release (`v0.1.0`)

---

## Recommended reading
- *Crafting Interpreters* — Robert Nystrom (free online; the best starting point)
- *Writing an Interpreter in Go* / *Writing a Compiler in Go* — Thorsten Ball
- *Engineering a Compiler* — Cooper & Torczon
- *Types and Programming Languages* — Benjamin Pierce (for type-system depth)
