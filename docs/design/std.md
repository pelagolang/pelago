# The standard library (`std`)

Two packages ship with the compiler.

- **`core`** is always there. Its `pub` names (lists, text, maps, tasks, ...) are the prelude. It is split into one module per file in `compiler/core/` (listed in `package.rs`).
- **`std`** holds the larger libraries. It is added to a program only when a module imports `std.<name>`, so programs that do not use it pay nothing in compile time. Files live in `compiler/std/`, one module per file.

| Module | What it is |
|---|---|
| `std.big` | `bigint`: integers of any size (sign and base 10^9 limbs). `+ - * / %` (rounding toward zero), `pow`, `divRem`, `negate`, `abs`, `sign`, `toInt`, comparisons, `show`. `big.fromInt(n)` and `big.parse(text)` build one. |
| `std.json` | `json.parse(text)` reads a document into a `json.value` (`nothing`, `flag`, `number`, `text`, `items`, `fields`); `encode()`, `pretty(indent)`, `get(key)`, `at(i)`, `asNumber`, `asText`, `asBool`, `size`. Numbers are floats, objects keep key order, `\u` escapes and surrogate pairs work. Typed decoding into your own structs waits for `meta` reflection. |

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
