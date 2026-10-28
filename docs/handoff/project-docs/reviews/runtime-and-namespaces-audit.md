# Runtime and namespace audit (2026-10-23, read from main + built `pelago`, no code changed)

## 1. Is the runtime designed?

Short answer: **the machine runtime is built and documented; the library runtime is built but only stage 0; the OS-facing runtime (I/O, args, time, FFI, real threads) is mostly not designed.** Compile time is far ahead.

### Designed AND implemented
- **Process start, exit, stop, output, allocator**: `docs/design/backend.md` section 3 and `native/rt.rs` (x86-64 Linux/Windows) and `rt_a64.rs` (ARM Linux/Windows/macOS). No libc. Size-class free lists (32 B to 64 KiB), 1 MiB chunks from mmap, large blocks mapped directly, 64 KiB print buffer, a 1 GiB own stack with a guard zone, and signal/exception handlers that turn stack overflow and bad addresses into `stopped: ...`. Leak counter in test builds.
- **Stops and traps**: division by zero, overflow, shift width, bounds all `Stop(msg)` with exit 1. The IR interpreter is the specification and a differential test compares native output, message and status.
- **Ownership at run time**: drops, temporaries ledger, `box`, `deinit`, leak-free (memory.md, A-03).
- **Lists, strings, maps/sets, sort, text search, objects/`any` tables, closures**: written in Pelago itself in `compiler/core/core.pel` (2271 lines) over about 25 `rt_*` intrinsics (alloc/free/load/store/print/copy). This is a good design: the "runtime" is ordinary checked library code that the optimiser can inline.
- **Tasks, channels, `shared`, `lock`, `select`, `scope`, `async`/`await`**: semantics decided (concurrency.md CC1 to CC10) and working, **but run sequentially** (S-02). `spawn` queues the task and runs it when joined/needed or at the end of `main`. Ping-pong between two tasks is impossible today.

### Only implicit in the code (no design page)
- **Panic/`assert`/`unreachable` behaviour**: it is `Stop` with a message and exit 1. There is no unwinding (right, no exceptions), but no decision on: a panic inside a task, `defer`/`deinit` on panic (do they run? today: no, the process stops), a backtrace/location in the message, exit codes beyond 1.
- **Map layout**: described in a comment block in core.pel and card S-01 (removal is O(n); a better design is written but not built).
- **Value layouts/ABI** of strings, lists, options, results, closures: in `ir.md` section 8 and the lowering; fine for us, but not stable or documented as an FFI ABI.
- **Float printing**: 6-digit stage-0 format (S-03 #12).

### Missing (not designed)
- **Real I/O**: only `print` (stdout) exists. No stdin, no files, no `read_file`, no `stderr`, no directories, no exit code from `main`. `errors.md` uses `read_file(path)?` in examples but nothing implements it. `ROADMAP.md` lists "File and console I/O" as unchecked.
- **`main(args: [string])`**: the checker accepts it, but running it stops with `expected an address` (I tried `fn main(args: [string]) { print(args.len()) }`). Args and environment are not passed at start. This is a real bug / gap.
- **Time, clocks, sleep, random, `env`**. `select` with timeout and `ms(50)` are in the design with no clock behind them.
- **FFI/C interop**: `ext fn` exists but means "the backend provides it" (library intrinsics). `modules.md` section 7 sketches `ext "C"` linking, but the backend has no linker, no libc and no dynamic imports on Linux (Windows imports kernel32 only). Calling C or any OS API beyond the 8 kernel32 functions is not designed. This is the biggest runtime decision ahead: static syscalls only, or allow libc/`dlopen`.
- **Real threads**: S-02 (atomics in the IR, `clone`/`futex`, thread-safe allocator, detached task policy). Allocator is single-threaded.
- **Async suspension**: S-04 (today `await` runs to the end).
- **Standard library**: there is no `std` package at all. `import std.io` fails with "there is no module or package `std`". Everything is in the always-visible `core`, which is `pub`-everything-in-scope. No plan yet for what is `core` (prelude) vs `std` (imports).
- **Allocator customisation** (arenas/custom allocators), **stack size policy**, **signals**, **process spawn**.
- **Float printing** shortest round-trip, **UTF-8/unicode** tables (case, width) and string formatting width/precision.

So: the answer to "are we only designing compile time?" is yes for everything that touches the world outside the process. The in-process runtime (memory, collections, traps) is solid; the OS-facing half is a blank page.

## 2. How the namespace system works (checked by running `pelago check/run` on a small project)

- **A file is a module named after the file**; a directory is a parent module; the **front file** `geometry.pel` next to `geometry/` is the parent. A missing front file becomes an empty made-up module. Order and cycles do not matter (two passes). `mod name { }` makes inline modules.
- **A package is a directory** passed to `pelago check/run/build`; its name is the directory name (`main` if you pass `.`). A single `.pel` file is a package of one module, so **it cannot see its siblings**.
- **Imports are bindings** (decided IM1): 
```
import geometry                          // module under its last name: geometry.unit()
import g = geometry.circle               // renamed:  g.circle_area(2.0)
import { shout } = util.strings          // names:    shout("hi")
import { read, put: write } = .sibling   // rename one; `...` for the rest
pub import { circle_area } = .geometry.circle   // re-export
```
  Verified: all of these compile and run in a directory project. `as` is not supported (by design: `import x as y` is a parse error).
- **Paths**: first segment is a top-level module of your package, or a package name (including your own), or a leading dot (`.sibling`, `..up`). `core` is always in scope without an import.
- **Visibility**: `priv` default (module + submodules), `pub pkg` (package only), `pub`. Verified messages: "`hidden` is private to `main.geometry.circle`", "`hidden` is not exported by ...".
- **Clashes**: two imports of the same name give an error at the first use naming both ("`one` could be `main.d.x.one` and `main.d.y.one`; write the module name in front"). A local declaration beats an import (design says "with a warning"; I did not see a warning from `check`).
- **Orphan rule** for `impl` is enforced.
- **Diagnostics** show `module:line:col` where module is the dotted path (`c:1:14`), not a file path.

### Gaps and surprises
1. **No `std` and no dependencies**: the design says the first segment may be `std` or a dependency, but `pelago` takes one path and loads only `core` plus that package. There is no way to depend on another package, no manifest (`pelago.toml` only holds `[fmt]`), no package manager (ROADMAP Phase 7).
2. **Running a single file loses imports**: `pelago run c.pel` where `c.pel` has `import .a` fails with "there is no module `a` there" even though `a.pel` is next to it. You must run the directory. This will surprise people and editors (the language server sees one file at a time unless it is told the root).
3. **Entry point is by directory, not by file**: how does a directory with two files that both have `main` behave? Not specified.
4. **Package vs top-level-module name clash**: in a project run as `.`, the package is called `main` and `import main.geometry` resolves `main` as the module `main.pel` first, giving "`main.main` has no module or item `geometry`". Naming the package after the directory makes this order-dependent.
5. **Relative `.x` means sibling of the current module**, not child, and children are reached by the full path; `..` goes up directory. It matches modules.md IM4 but is the inverse of Python/TS habits.
6. **File names are module names**: renaming a file breaks importers; names must be valid on case-insensitive disks (decided and accepted in modules.md).
7. **Re-exports**: the front-file pattern works (`pub import` from `geometry.pel`), and cycles (a imports b imports a) are accepted.
8. **`core` is flat and everything `pub` there is global**: adding any `pub` name to core can break user programs that use the same name (a local wins, but only with a warning). Needs the prelude/std split before a 1.0.

## 3. Recommended next steps

Runtime:
1. **Decide the OS interface story (the key decision)**: raw syscalls only (no libc, matches today), or optional libc/dlopen for FFI. This shapes `ext "C"`, I/O, time and threads. I recommend raw syscalls for the `std` core (files, stdin/stderr, time, args, exit), and a separate, explicit `ext "C"` mechanism for outside libraries later.
2. **Fix `main(args)`** and add `exit(code)`, a `main` that returns an exit status, and stderr (`eprint`). Small, unblocks real programs.
3. **Write `docs/design/runtime.md`** collecting: start-up, panic/trap/exit-code policy (does `defer`/`deinit` run on panic, panic in a task), stdio and files (`io_error`), time and random, args/env, allocator contract, FFI ABI, and the std/core split.
4. **Std design**: `core` (prelude, interfaces, collections) vs `std.io`, `std.fs`, `std.time`, `std.env`, `std.os`. Give `import std.x` a meaning (a bundled package next to `core`).
5. Then S-02 (real threads) and S-01 (map removal), both already carded, after the I/O decisions.

Namespaces:
6. Make `pelago run/check <file>` find its project root (walk up to `pelago.toml`), and use the same root in the language server.
7. Add `[package]` and `[dependencies]` to `pelago.toml` (name, version, path/git deps) and have the first segment resolve to them. Settle the package-name-vs-module-name clash (suggest: package names are always shadowed by nothing; reject a top-level module with the package's name).
8. Make the "local beats import" warning visible in `check` (or make it an error), and test the `pub pkg` and orphan cases in docs.

Benchmarks (your first paragraph):
- **C++ is not faster than C** for equivalent code; both go through gcc's optimiser. A C++ column mostly measures `std::` containers (std::map/unordered_map/string) rather than the language, so it is useful for the map/string/sort benches and a reasonable "what people would write" baseline. Worth adding `g++ -O2` twins for those.
- **tcc is fast to compile, not fast to run** (it does almost no optimisation, so it is roughly like `gcc -O0`). The "10x faster than gcc" is about compile time. So tcc belongs in a **compile-time benchmark** (our `pelago build` vs tcc vs gcc -O0/-O2 vs rustc), which is also in the ROADMAP's targets, not in the run-time table. I would add that as a second suite.
- Making the suite as mature as the goose results page (I did not read it; I can next time you ask): the current suite is 16 programs (`compiler/benches/`), C twins for all, Rust twins for some. Ideas to mature it: more programs from known sets (nbody, spectral-norm, fannkuch, binary-trees, mandelbrot, json, hash-map heavy), the same program in several languages in one table, geomean per category, a checked (trapping) and unchecked mode, compile-time and binary-size columns, results committed as a generated `results.md` by the weekly routine.

Questions for you:
1. OS interface: raw syscalls only for `std`, with a separate explicit FFI later (my recommendation), or libc from day one?
2. Do you want `defer`/`deinit` to run on panic (unwinding-lite) or panic = immediate stop?
3. Should I plan the runtime design page and `std` split next (docs only), or fix `main(args)` plus `eprint`/`exit` first?
