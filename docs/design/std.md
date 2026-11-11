# The standard library (`std`)

Two packages ship with the compiler.

- **`core`** is always there. Its `pub` names (lists, text, maps, tasks, ...) are the prelude. It is split into one module per file in `compiler/core/` (listed in `package.rs`).
- **`std`** holds the larger libraries. It is added to a program only when a module imports `std.<name>`, so programs that do not use it pay nothing in compile time. Files live in `compiler/std/`, one module per file.

| Module | What it is |
|---|---|
| `std.big` | `bigint`: integers of any size (sign and base 10^9 limbs). `+ - * / %` (rounding toward zero), `pow`, `divRem`, `negate`, `abs`, `sign`, `toInt`, comparisons, `show`. `big.fromInt(n)` and `big.parse(text)` build one. |
| `std.io` | `io.eprint(text)`, `io.eprintln(text)` (standard error, unbuffered, after flushing standard output), `io.readAll()` (the rest of standard input) and `io.readLine()` (`string?`, `none` at the end, `\r\n` and `\n` removed). Also `io.readBytes()`, `io.tryReadLine()`, `io.tryReadAll()` and the `file` type. All targets except `main(args)` (see `input.md`). |
| `std.fs` | `fs.readText`, `fs.readBytes`, `fs.writeText`, `fs.appendText`, `fs.open`, `fs.create`, `fs.append`: files with `io_error` as a value (`docs/design/input.md`). |
| `std.os` | `os.exit(code)`: ends the program at once, flushing output; `defer` and `deinit` do not run. The arguments are the parameter of `fn main(args: [string])`. |
| `std.path` | Paths as text, no file access: `join`, `fileName`, `parent`, `stem`, `extension`, `withExtension`, `isAbsolute`, `components`, `normalise`. Both `/` and `\\` separate components; built paths use `/`. |
| `std.json` | `json.parse(text)` reads a document into a `json.value` (`nothing`, `flag`, `number`, `text`, `items`, `fields`); `encode()`, `pretty(indent)`, `get(key)`, `at(i)`, `asNumber`, `asText`, `asBool`, `size`. Whole numbers that fit `int` stay exact (`integer`), other numbers are floats, parse errors say `(line 3, column 9)`, objects keep key order, `\u` escapes and surrogate pairs work. Decoding into your own types: `value.toInt`, `toRanged`, `toFloat`, `toBool`, `toText`, `indexOf`, `mismatch`, `missing`, `json_error` with `path`, `line`, `column`, `json.document`, `json.located` (the reader layer of `docs/design/json.md`, card S-08; `@derive(fromJson)` writes calls to these: `serverFromJson(value, inout at)` and `serverDecode(text)` for a type `server`, see `examples/programs/config.pel`). |
| `std.regex` | `regex.compile(pattern)` builds a matcher (a Pike VM: time grows with text times pattern, never exponentially). `isMatch`, `find`, `findAll`, `count`, `replaceAll` (`$0`-`$9`, `$$`), `found.group`. Literals, `.`, classes, `\\d \\w \\s \\b` and their negations, `^ $`, groups, `(?:)`, `|`, `* + ? {n,m}` and lazy forms. Works on bytes; no flags, no lookaround, no backreferences. Checked against Python's `re`. |

Use:

```
import std.big
import { bigint } = std.big

const a = big.parse("123456789012345678901234567890")?
print(a * a, a.pow(3))
```

Why a module is also a type name: the module is `big` and the type is `bigint`, so both can be in scope.

Naming follows decision B8 (`docs/design/basics.md`): camelCase functions and methods.

Tests: each library has a `tests/` directory (`compiler/core/tests/`, `compiler/std/tests/`) of `.pel` programs with their expected output in `name.out`; `name.stops` marks a program that must stop. The test `library_test_suites` runs them all. The `bigint` expectations come from Python's integers.
