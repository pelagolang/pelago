# Modules and imports (draft 2)

What a file is, how names cross files, and what `import` looks like. This is the last syntax piece the type
checker needs before name resolution can be written.

**Already decided:** `mod` for modules, `pkg` reserved for bundles, `import` (with `use` as another spelling),
`pub` / `priv` with private as the default, `export` as another spelling of `pub`, order inside a module does not
matter. **Parser today:** `import std.io` and `import a, b from std.io` as placeholders (parser.md, P2).

## 1. Files, modules, packages

- **A file is a module.** Its name is the file name: `geometry.lang` is the module `geometry`. A directory of files is
  a module too, with its files as submodules (`shapes/circle.lang` is `shapes.circle`). No `mod name` line is needed;
  if it is written (`mod tour`), it must match the file name, so a copied file cannot silently change its name.
- **`mod name { ... }`** declares a small module inside a file, for grouping and for tests.
- **A package** (`pkg`, reserved) is a directory tree that is built and shared as one unit, with a manifest. It is the
  unit other people depend on. Its design belongs with the build tool (Phase 7); here we only need the rule that the
  first segment of an import path is either a package name (`std`, a dependency, or the current package) or a
  relative start (section 3).
- **Order does not matter** inside a module, and modules may import each other (section 5).

## 2. The import form

Three jobs: bring in a module, bring in some of its names, bring in a module under another name. The question is
how to write them in our own way.

**Option A: keep the JS/Python family shape.**

```
import std.io                       // use as io.read()
import read, write from std.io      // use as read(), write()
import std.io as fmt                // needs a new word: `as`
```

**Option B (recommended): an import is a binding, written like the declarations we already have.**

```
import std.io                       // the module, under its last name: io.read()
import fmt = std.io                 // the module under another name: fmt.read()
import { read, write } = std.io     // some names, brought in directly
import { read, write: put } = std.io    // rename one, as in a struct pattern
import { ... } = std.math           // everything it makes public
```

Why B: it reuses grammar you already know (`type id = int64`, `const point { x, y } = p`, `...` for "the rest"), it
needs no new keyword (`as` is not needed, and `from` becomes unnecessary), and the shape is the same for every job:
`import <what you get> = <where from>`. The plain `import std.io` form is the short version of `import io = std.io`.
`use` remains an exact synonym.

Tokens are about equal: `import { read, write } = std.io` is the same length as `import read, write from std.io`.

Re-exporting: `pub import { read } = std.io` makes the imported names part of this module's public face
(`export import` also works, since `export` is `pub`).

## 3. Paths

| Path | Meaning |
|---|---|
| `std.io` | first segment is a package: the standard library |
| `geometry.shapes` | first segment is the current package's top-level module |
| `.sibling` | starts with a dot: relative to this file's directory (a sibling file or submodule) |
| `..util.strings` | `..` goes to the parent directory |

Leading dots echo the leading-dot enum cases (`.circle`): "look from here". An absolute path inside your own package
starts with the package name, so moving a file does not break the imports of others.

## 4. Visibility

- `priv` (the default) is visible inside the module, its submodules and `test` blocks.
- `pub` is visible to every module that can reach this one.
- Question PV1 below: do we want a middle level, "visible inside my package but not outside", for the library
  that ships many modules but offers a small public face? Rust has `pub(crate)`, Go has capitalisation plus
  `internal/`, Java has package-private. Suggested spelling if wanted: `pub pkg` (two words that already exist).

## 5. Cycles and speed

- Modules in one package **may import each other**. Name resolution runs in two passes (collect all names, then
  resolve uses), so order and cycles do not matter. The only forbidden cycle is a type that contains itself by value
  (that is a size problem, reported by the type checker).
- Because every file is a module and imports are explicit, each file can be parsed and its names collected in
  parallel, which is the plan for compile speed.
- Imports are allowed only at the top level of a module (or `mod` block), not inside functions: it keeps resolution
  one table per module.

## 6. Names

- **The prelude** (always in scope without an import): the built-in types (`int`, `string`, `option`, `result`, ...),
  `ok`, `err`, `some` (as a pattern), `print`, `assert`, `panic`, `todo`, `unreachable`.
- A name brought in by `{ ... }` and a local declaration with the same name: the local one wins, with a warning.
- Two imports bringing the same name: an error at the first use (naming both), not at the import. Writing the
  module-qualified name (`io.read`) always works.
- Imported names are not re-exported unless the import is `pub`.
- **Methods and interfaces:** a type's methods come with the type. An `impl` for a type from another package is
  allowed only if the interface or the type is yours (the orphan rule), so two packages cannot give the same type the
  same method.

## 7. C and other outside code

`ext` declarations (`ext fn puts(s: ptr) -> int`) are not imports. They are listed where the library is linked
(`ext "C" ...`), and the build tool links the library. Header parsing is out of scope.

## Decisions from draft 1

- **IM1** Imports are bindings (Option B): `import std.io`, `import fmt = std.io`, `import { a, b: c } = path`.
  The parser, printer and checks implement it; `use` is a synonym, and `pub import {...}` re-exports.
- **IM2** `import { ... } = std.math` brings in everything the module makes public. `...` can sit next to renames
  (`{ read: get, ... }`).
- **IM5** Modules of one package may import each other.
- **IM6** Imports only at the top level of a module (or a `mod` block).
- **IM7 / PV1** A middle visibility level, spelled `pub pkg`: visible inside the package, not outside. (Checked
  once name resolution exists; the parser does not know `pub pkg` yet.)
- **IM8** The orphan rule for `impl`.

## Still open (waiting on the answer about how a file becomes a module)

- **IM3** What decides a module's name: the file name, a declared name, or the directory.
- **IM4** Relative paths with leading dots (`.sibling`, `..parent.x`). The parser accepts them already; whether
  they stay depends on IM3.

### Why "file name = module name", and the alternatives

It is not a Java idea (Java is one *class* per file). Whole-file namespaces named after the file are how JavaScript,
Python, Rust, Zig and Swift work. The reasons to like it here:

- **Finding a module needs no scan.** `import geometry.shapes` can be turned into a path and opened directly. If names
  were only declared inside files, the compiler would have to read every file first to learn what exists.
- **Parallel by default.** One file, one module, one unit to parse and collect names for.
- **Nothing to keep in step.** The name cannot disagree with the file (hence the rule that a `mod tour` line must match).

The real costs: renaming a file renames the module (a refactor for everyone importing it), a big module has to be one
big file or a folder of submodules re-exported by a front file (`pub import { ... } = .list`), and file names must be
valid names on every operating system (case-insensitive disks, no dashes).

The serious alternative is **Go-style: a directory is a module and all its files share one namespace**.

| | File = module | Directory = module (Go) |
|---|---|---|
| Splitting a big module across files | needs submodules and re-exports | free: just add a file |
| Private names across files of one module | not visible | visible |
| Finding a module | open one file | read the directory's file list |
| Parallel unit | file | file (parse), directory (collect names) |
| Renaming a file | breaks importers | harmless |

I suggest keeping **file = module**, with directories as parent modules whose front file (`shapes.lang` next to
`shapes/`) can re-export its children: it matches what you already know from TypeScript and Rust, and `pub import`
makes the facade cheap. The Go style is the better answer if you expect very large modules and want to split them
without ceremony. A third option, **a declared name that is free of the file name** (`mod geometry` inside any file),
is the most flexible and costs the directory scan above.

**IM3 (again).** Which one: file = module (suggested), directory = module, or declared name?
**IM4 (again).** After IM3, keep leading-dot relative paths?
