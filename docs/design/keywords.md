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

Decided: type names are lowercase, and sized integers run from 8 to 64 bits.

| Name | Meaning | Phase |
|---|---|---|
| `int` | Whole number, signed. Size still being decided (see basics.md, B2) | 1 |
| `int8` `int16` `int32` `int64` | Signed whole numbers of an exact size | 2 |
| `uint` `uint8` `uint16` `uint32` `uint64` | Unsigned whole numbers (zero and up) | 2 |
| `float` | Decimal number, 64-bit | 1 |
| `float32` `float64` | Decimal numbers of an exact size | 2 |
| `bool` | `true` or `false` | 1 |
| `char` | One character | 1 |
| `string` | Text | 1 |
| `[T]` | A growable list of `T` | 2 |
| `option<T>` | Either a `T` or `none` | 2 |
| `result<T, E>` | Either a success `T` or an error `E` | 2 |

Open: do types you define yourself also start lowercase, or with a capital
(like Python, where built-ins are lowercase and your own classes are `CapWords`)?

## More candidates

Everything else I can think of, including the words we are deliberately *not*
taking. **Lean:** Include / Maybe / Leave out.

### Types and structure

| Word | Meaning | Seen in | Lean | Why |
|---|---|---|---|---|
| `class` | Blueprint for objects, with inheritance and shared references | Java, C++, Python, TS | Leave out | `struct` plus interfaces covers it without inheritance or sharing |
| `trait` / `protocol` | Other names for `interface` | Rust / Swift | Rename option | Pick one name for the concept |
| `extends` / `inherits` | One class builds on another | Java, TS, Python | Leave out | No implementation inheritance |
| `implements` | A class promises an interface | Java, TS | Leave out | `impl` does this |
| `abstract` `virtual` `override` `final` `sealed` | Inheritance machinery | Java, C++, C# | Leave out | Only needed with inheritance |
| `union` | One memory slot that can hold one of several layouts | C | Maybe, phase 3 | Needed for C interop and low-level work; `enum` covers the safe case |
| `namespace` / `package` | Groups of declarations | C++, Java | Leave out | `module` covers it |
| `export` / `from` / `use` | Other spellings of `pub` and `import` | JS, Python, Rust | Rename option | |
| `static` | A value that lives for the whole program, or belongs to a type | C, Java | Maybe | Global state is awkward under value semantics, but sometimes needed |
| `public` `private` `protected` | Visibility levels | Java, C++ | Leave out | `pub` with private by default is enough; `protected` only matters with inheritance |
| `macro` | Code that writes code | Rust, C | Leave out | `comptime` covers the useful cases |
| `operator` | Define how `+`, `==` and so on work for your type | C++, Swift | Maybe | Probably through interfaces rather than a keyword |
| `init` | Constructor: how a value is first created | Swift, Python (`__init__`) | Maybe | Struct literals may be enough |
| `deinit` | Cleanup that runs when a value goes away (frees memory, closes files) | Swift, C++ destructors | Include, phase 3 | With no GC, this is how owning types clean up |
| `this` | Another name for `self` | Java, C++, JS | Rename option | |
| `new` / `delete` | Allocate and free memory by hand | C++, Java | Leave out | Allocation goes through the standard library |

### Variables and types

| Word | Meaning | Seen in | Lean | Why |
|---|---|---|---|---|
| `let` / `val` / `mut` | Other spellings for the `const`/`var` pair | Rust, Swift, Scala | Rename option | |
| `ref` | A reference stored as a value | C#, Rust | Leave out | References are temporary under value semantics |
| `null` / `nil` / `undefined` | "No value here" | everywhere | Leave out | `none` in `option<T>` instead, so absence is visible in the type |
| `void` | A function that gives back nothing | C, Java | Maybe | Could be an empty value type instead |
| `never` | A function that never returns (crash, infinite loop) | TS, Rust | Maybe, phase 3 | |
| `typeof` `sizeof` `alignof` | Ask for a type, its size, its alignment | JS, C | Maybe | Likely `comptime` functions rather than keywords |
| `volatile` `atomic` | Memory the hardware or other threads can change | C, C++ | Maybe, phase 3 | Needed for low-level and concurrent code |

### Control flow

| Word | Meaning | Seen in | Lean | Why |
|---|---|---|---|---|
| `else if` / `elif` | Chain conditions | C / Python | Use `else if` | No extra keyword |
| `then` / `do` / `end` | Block delimiters without braces | Pascal, Lua, Ruby | Leave out | We chose braces |
| `repeat` ... `until` | Loop that tests at the end | Pascal, Lua | Leave out | `loop` with `break` covers it |
| `goto` | Jump to a label | C | Leave out | Hard to read and to optimize |
| `try` / `catch` / `throw` / `finally` | Exceptions | Java, JS, Python | Leave out | Hidden control flow; harder to compile well. `result` and `?` instead |
| `guard` | "Leave early unless this is true" | Swift | Maybe | Reads very well for beginners |
| `fallthrough` | Continue into the next `case` | C, Swift | Leave out | No silent fallthrough; each case stands alone |
| `yield` | Produce values one at a time from a function | Python, JS | Maybe, later | |
| `with` | Scoped resource handling | Python | Leave out | `deinit` and `defer` do this |
| `unless` | Opposite of `if` | Ruby | Leave out | `if not` is enough |

### Operators written as words

| Word | Meaning | Seen in | Lean | Why |
|---|---|---|---|---|
| `xor` | Exclusive or | Python (`^`) | Maybe | |
| `mod` / `rem` | Remainder after division | Pascal | Leave out | `%` |
| `instanceof` | Is this value of that class | Java, JS | Leave out | `is` covers it |

### Low level and concurrency

| Word | Meaning | Seen in | Lean | Why |
|---|---|---|---|---|
| `spawn` / `go` | Start a concurrent task | Go, Rust | Maybe, later | Concurrency is out of scope for v0 |
| `select` / `chan` | Wait on several channels / the channel type | Go | Maybe, later | |
| `asm` | Inline assembly | C, Rust | Maybe, later | Needed for a runtime or kernel work |
| `inline` / `noinline` | Hints to the optimizer | C, Rust | Maybe, later | |
| `align` / `packed` | Control memory layout | C, Zig | Maybe, later | |

### Diagnostics

| Word | Meaning | Seen in | Lean | Why |
|---|---|---|---|---|
| `assert` | Crash if this condition is false | C, Python | Include | Probably a built-in function |
| `panic` | Crash now with a message | Rust, Go | Include | Probably a built-in function |
| `unreachable` | Tell the compiler this point can never be reached | Rust, Zig | Maybe | Lets the optimizer assume more |
| `todo` | Placeholder that crashes if reached | Rust | Maybe | Handy while writing |

## Decisions that affect the table

**`match` vs `switch`.** Pattern matching can do everything a plain `switch` does,
so two separate constructs would be two ways to do the same thing. The cleaner design
is one construct with the familiar `switch`/`case` look and full pattern-matching
power (Swift does exactly this, and Java 21 and Kotlin are moving the same way).
The table above assumes that. Say so if you want two constructs anyway.

**`and`/`or`/`not` or `&&`/`||`/`!`?** Words read like English and match Python.
Symbols are shorter and match C, JS, Rust and TS.
