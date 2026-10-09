# Keyword table (working names)

Every word the language might reserve, what it means, and where it comes from.
**Working** is my default. Write your rename in the last column.
**Phase:** 1 = needed for the basics, 2 = core language, 3 = later.

## Declarations

| Working | Meaning | Seen in | Phase | Your rename |
|---|---|---|---|---|
| `var` | A variable that can be changed after it is created | Pascal, JS, Go, Swift, Kotlin | 1 | |
| `const` | A variable that cannot be changed after it is created (fully immutable) | C, JS, Go, Zig | 1 | |
| `fn` | Declares a function | Rust, Zig | 1 | |
| `struct` | A bundle of named fields, a custom type that is a value | C, Rust, Go, Swift | 2 | |
| `enum` | A type that is exactly one of several named options, each option may carry data | C, Rust, Swift | 2 | |
| `interface` | A list of functions a type promises to provide | Java, Go, TS | 2 | |
| `impl` | States that a type provides an interface | Rust | 2 | |
| `type` | Gives a new name to an existing type | TS, Go, Rust | 2 | |
| `module` | A named group of declarations, usually one file | Python, Rust (`mod`) | 2 | |
| `import` | Brings names from another module into scope | JS, Python, Java | 2 | |
| `pub` | Makes a declaration visible outside its module (private is the default) | Rust | 2 | |
| `comptime` | Runs code while the program is being compiled | Zig | 3 | |
| `extern` | Declares something defined outside the language, for example a C function | C, Rust | 3 | |

## Control flow

| Working | Meaning | Seen in | Phase | Your rename |
|---|---|---|---|---|
| `if` / `else` | Choose what runs based on a true or false condition | everywhere | 1 | |
| `switch` / `case` / `default` | Choose by comparing a value against patterns (see decision below) | C, Swift | 1 | |
| `for` ... `in` | Repeat once for each item of a collection or range | Python, Rust, Swift | 1 | |
| `while` | Repeat as long as a condition is true | C, Python | 1 | |
| `loop` | Repeat forever until `break` | Rust | 2 | |
| `break` | Leave the loop now | C, Python | 1 | |
| `continue` | Skip to the next round of the loop | C, Python | 1 | |
| `return` | Leave the function now, optionally giving back a value | Fortran, C | 1 | |
| `defer` | Run this when the current block ends, however it ends (good for cleanup) | Go, Zig, Swift | 3 | |
| `where` | Adds conditions to a generic ("T must be Ordered") | Rust, Swift, Haskell | 3 | |

## Values and names

| Working | Meaning | Seen in | Phase | Your rename |
|---|---|---|---|---|
| `true` / `false` | The two Boolean values | everywhere | 1 | |
| `none` | "No value here", only for optional types. There is no null | Swift, Rust (`None`), Python (`None`) | 2 | |
| `self` | The value a method is working on | Smalltalk, Python, Rust, Swift | 2 | |
| `Self` | The type of `self` inside a type or interface | Rust, Swift | 3 | |

## Value-semantics access modes (the new part)

| Working | Meaning | Seen in | Phase | Your rename |
|---|---|---|---|---|
| `inout` | Function may change the caller's value, exclusive access for the duration of the call | Ada, Swift, Hylo | 2 | |
| `sink` | Function takes the value away from the caller and consumes it | Hylo, Nim | 2 | |
| `copy` | Make an independent duplicate of an owning value | Swift | 2 | |
| `any` | Marks a type as "any type that provides this interface" (dynamic dispatch) | Swift | 3 | |

## Words used as operators

| Working | Meaning | Seen in | Phase | Your rename |
|---|---|---|---|---|
| `and` / `or` / `not` | Logical and, or, not. The alternative is `&&`, `||`, `!` | Python | 1 | |
| `is` | Test whether a value is a particular enum case or type | Python, C#, Kotlin | 3 | |
| `as` | Convert a value to another type | Rust, TS, Swift | 2 | |

## Safety and the edges

| Working | Meaning | Seen in | Phase | Your rename |
|---|---|---|---|---|
| `unsafe` | Marks a block where the compiler's safety checks are switched off | Rust, C# | 3 | |
| `async` / `await` | Concurrency without blocking a thread | JS, Rust, C# | 3 | |
| `test` | A built-in test block that only compiles in test mode | Zig | 3 | |

## Type names (not keywords, but reserved in the standard library)

| Working | Meaning | Phase |
|---|---|---|
| `Int` | Whole number, signed, the size of a machine word (64 bits on a 64-bit CPU) | 1 |
| `Int8` `Int16` `Int32` `Int64` | Signed whole numbers of an exact size | 2 |
| `UInt` `UInt8` ... `UInt64` | Unsigned whole numbers (zero and up) | 2 |
| `Float` | Decimal number, 64-bit | 1 |
| `Float32` `Float64` | Decimal numbers of an exact size | 2 |
| `Bool` | `true` or `false` | 1 |
| `Char` | One character | 1 |
| `String` | Text | 1 |
| `[T]` | A growable list of `T` | 2 |
| `Option<T>` | Either a `T` or `none` | 2 |
| `Result<T, E>` | Either a success `T` or an error `E` | 2 |

## Decisions that affect the table

**`match` vs `switch`.** Pattern matching can do everything a plain `switch` does,
so two separate constructs would be two ways to do the same thing. The cleaner design
is one construct with the familiar `switch`/`case` look and full pattern-matching
power (Swift does exactly this, and Java 21 and Kotlin are moving the same way).
The table above assumes that. Say so if you want two constructs anyway.

**`and`/`or`/`not` or `&&`/`||`/`!`?** Words read like English and match Python.
Symbols are shorter and match C, JS, Rust and TS.
