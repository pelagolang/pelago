# Modules and imports (draft 1)

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

## Questions

**IM1.** Take Option B (imports as bindings: `import { a, b: c } = path`, `import name = path`, `import path`)?
**IM2.** `{ ... } = std.math` for "everything", or leave glob imports out entirely (always name what you use)?
**IM3.** File name is the module name, and a `mod` line, if written, must match?
**IM4.** Relative paths with leading dots (`.sibling`, `..parent.x`)?
**IM5.** Cycles between modules of one package allowed (section 5)?
**IM6.** Imports only at module top level?
**IM7.** A middle visibility level (PV1), spelled `pub pkg`? Or only `pub` and `priv` for now?
**IM8.** The orphan rule for `impl` (section 6)?
