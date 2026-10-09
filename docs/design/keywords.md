# Keyword table (working names), version 2

Every word and symbol the language might reserve. **Working** is the current
choice. Write renames in the last column.
**Phase:** 1 = needed for the basics, 2 = core language, 3 = later.

## Decisions so far

- Operators are **symbols only**, for token efficiency: `&&` `||` `!` `^` `%`. No `and`, `or`, `not`, `xor`, `mod`.
- Access modes (`let`, `inout`, `sink`) apply **everywhere a name is bound**: parameters, loop variables, `switch` patterns, closure captures.
- `ext` replaces `extern`. `priv` joins `pub`. `fn` is final.
- `module` vs `pkg`, and a new name for `comptime`, are still open.
- Inheritance is being reconsidered, see "Types and structure".

---

## Declarations

| Working | Meaning | Seen in | Phase | Your rename |
|---|---|---|---|---|
| `var` | A variable that can be changed after it is created | Pascal, JS, Go, Swift, Kotlin | 1 | |
| `const` | A variable that cannot be changed (fully immutable) | C, JS, Go, Zig | 1 | |
| `fn` | Declares a function | Rust, Zig | 1 | |
| `struct` | A bundle of named fields, a custom type that is a value | C, Rust, Go, Swift | 2 | |
| `enum` | A type that is exactly one of several named options, each may carry data | C, Rust, Swift | 2 | |
| `interface` | A list of functions a type promises to provide | Java, Go, TS | 2 | |
| `impl` | States that a type provides an interface | Rust | 2 | |
| `type` | Gives a new name to an existing type | TS, Go, Rust | 2 | |
| `module` | A named group of declarations, usually one file (open: `mod`, `pkg`, `unit`) | Python, Rust (`mod`) | 2 | |
| `import` | Brings names from another module into scope | JS, Python, Java | 2 | |
| `pub` | Visible outside its module | Rust | 2 | |
| `priv` | Private to its module. This is the default, so writing it is optional | Swift (`private`) | 2 | |
| `ext` | Declares something defined outside the language, for example a C function | C (`extern`) | 3 | |
| *(name open)* | Runs code while the program is being compiled. Candidates: `bake`, `meta`, `pre`, `eval`, `build` | Zig (`comptime`) | 3 | |

## Conditionals

| Working | Meaning | Seen in | Phase | Your rename |
|---|---|---|---|---|
| `if` / `else` / `else if` | Choose what runs based on a true or false condition | everywhere | 1 | |
| `unless` | Runs the block when the condition is *false*. Replaces `if !` | Ruby | 1 | |
| `switch` / `case` / `default` | Choose by matching a value against patterns | C, Swift | 1 | |
| `guard` | "Leave early unless this is true" | Swift | 2 | |
| `is` | Test whether a value is a particular enum case or type | Python, C#, Kotlin | 3 | |

## Loops

| Form | Meaning | Seen in | Phase | Your rename |
|---|---|---|---|---|
| `for x in items { }` | Once for each item of a collection | Python, Rust, Swift | 1 | |
| `for i in 0..10 { }` | Count from 0 up to but not including 10. `0..=10` includes 10 | Rust, Swift | 1 | |
| `for i in 0..10 step 2 { }` | Count in jumps | Kotlin, BASIC | 2 | |
| `for (var i = 0; i < n; i += 1) { }` | The three-part loop from C, Java and JS: setup; test; step | C, Java, JS, TS | 2 | |
| `for i, x in items { }` | Index and item together | Python (`enumerate`) | 2 | |
| `for inout x in items { }` | Change each item in place (access modes) | new | 2 | |
| `while cond { }` | Repeat while a condition is true | C, Python | 1 | |
| `do { } while cond` | Run once, then repeat while true | C, Java, JS | 2 | |
| `loop { }` | Repeat forever until `break` | Rust | 2 | |
| `break` / `continue` | Leave the loop / skip to the next round | C, Python | 1 | |
| `break value` | Leave a `loop` and give back a value | Rust | 3 | |
| `outer: for ...` and `break outer` | Name a loop so an inner loop can leave the outer one | Java, Rust | 3 | |
| `defer` | Run this when the current block ends, however it ends | Go, Zig, Swift | 3 | |
| `else` on a loop | Python's "ran without `break`" | Python | - | Leave out: confusing |
| `repeat ... until` | Loop that tests at the end, in a Pascal spelling | Pascal, Lua | - | Leave out: `do ... while` covers it |
| `return` | Leave the function now, optionally giving back a value | Fortran, C | 1 | |

Why `for x in items` is the main form: no index to get wrong (no off-by-one),
fewer tokens, and it works with access modes. `for (i = 0; i < n; i++)` is
here too, because TS/JS programmers expect it and it can step in odd ways.
(JS also has `for...in` and `for...of`, and its `for...in` means something
different. Ours means "each item", like Python.)

## Values, literals and names

| Working | Meaning | Seen in | Phase | Your rename |
|---|---|---|---|---|
| `true` / `false` | The two Boolean values | everywhere | 1 | |
| `none` | "No value here", only for `option<T>`. There is no null | Swift, Rust, Python | 2 | |
| `42`, `1_000_000` | Whole numbers; underscores for readability | Rust, Python | 1 | |
| `0xFF` `0b1010` `0o17` | Hexadecimal, binary, octal numbers | C and most | 2 | |
| `3.14`, `1e-3` | Decimal numbers, scientific notation | everywhere | 1 | |
| `inf` / `nan` | Infinity / not-a-number (library constants) | many | 3 | |
| `'a'` | One character | C, Rust | 1 | |
| `"text"` | A string. Escapes: `\n` `\t` `\\` `\"` and `\#` for a literal hash | everywhere | 1 | |
| `#name`, `#(expression)` | Puts a value into a string | new | 1 | |
| `[1, 2, 3]` | A list | JS, Python | 2 | |
| `(1, "a")` | A tuple: a few values of different types | Python, Rust | 2 | |
| `0..10`, `0..=10` | A range | Rust | 1 | |
| `_` | "I don't care about this value" | Rust, Python | 2 | |
| `self` | The value a method is working on | Smalltalk, Python, Rust, Swift | 2 | |
| `Self` | The type of `self` inside a type or interface | Rust, Swift | 3 | |
| `super` | The parent type's version, if inheritance is added | Java, Python | 3 | |
| `// ...` | A comment to the end of the line | C++, Java | 1 | |
| `/* ... */` | A comment over many lines | C | 1 | |
| `/// ...` | A documentation comment | Rust | 2 | |

## Operators and symbols (symbols only, no word forms)

| Symbol | Meaning | Phase |
|---|---|---|
| `+` `-` `*` `/` | Add, subtract, multiply, divide | 1 |
| `%` | Remainder after division | 1 |
| `**` | Power (open: or a library function) | 2 |
| `==` `!=` `<` `>` `<=` `>=` | Comparisons | 1 |
| `&&` `\|\|` `!` | Logical and, or, not | 1 |
| `&` `\|` `^` `~` `<<` `>>` | Bitwise and, or, xor, not, shift left, shift right. `^` is also xor on booleans | 2 |
| `=` `+=` `-=` `*=` `/=` `%=` `&=` `\|=` `^=` `<<=` `>>=` | Assign, and assign after a calculation | 1 |
| `++` `--` | Add or subtract one. Open: Rust, Python and Swift dropped them | 2 |
| `..` `..=` | Range, end excluded / included | 1 |
| `.` | Reach inside a value or call a method | 1 |
| `?` | If this is an error or `none`, return it from the function now | 2 |
| `??` | "Use this default if there's no value" | 3 |
| `->` | Return type of a function | 1 |
| `to` | Convert: `x to float`. A word, because the symbol was taken | 1 |
| `#` | Value marker inside strings | 1 |
| `a ? b : c` | Ternary. Not included: `if` already gives a value | - |

## Access modes (the new part)

| Working | Meaning | Seen in | Phase | Your rename |
|---|---|---|---|---|
| `let` | Read-only access. The silent default. Optional to write | Hylo | 2 | |
| `inout` | May change the caller's value, exclusive access while it runs | Ada, Swift, Hylo | 2 | |
| `sink` | Takes the value away from the caller and consumes it | Hylo, Nim | 2 | |
| `copy` | Makes an independent duplicate of an owning value | Swift | 2 | |
| `any` | "Any type that provides this interface" (dynamic dispatch) | Swift | 3 | |

## Types and structure

**Inheritance is being reconsidered** (see the chat, and the open list at the end).
The `Decision` column reflects what you said, where it depends on inheritance.

| Word | Meaning | Seen in | Decision | Why |
|---|---|---|---|---|
| `class` | A reference-like type with inheritance. A staged, opt-in feature | Java, C++, Python, TS | Reserved, decide path | Keeps `struct` simple; adds a second kind of type |
| `interface` | Promise of functions | Java, Go, TS | Keep | `trait` and `protocol` are other names for the same thing; not needed |
| `extends` | Class builds on another class (open: `from`, `inherit`, `:`) | Java, TS, JS | Reserved with `class` | Inheritance |
| `implements` | A class promises an interface | Java, TS | Leave out | `impl` does this |
| `abstract` `virtual` `override` `final` `sealed` | Inheritance machinery | Java, C++, C# | Reserved with `class` | Inheritance |
| `union` | One memory slot holding one of several layouts | C | Include | C interop and low-level work; `enum` covers the safe case |
| `export` / `from` / `use` | Other spellings of `pub` and `import` | JS, Python, Rust | Include | |
| `static` | A value that lives for the whole program, or belongs to a type | C, Java | Include | Sort the awkward cases later |
| `public` `private` `protected` | Visibility. We write `pub` and `priv`; `protected` only matters with inheritance | Java, C++ | `protected` reserved | Short forms in use |
| `macro` | Code that writes code | Rust, C | Maybe | Overlaps with the compile-time word; keep only one |
| `operator` | Define how `+`, `==` and so on work for your type | C++, Swift | Include | |
| `init` | Runs when a value is first created: checks, defaults, computed fields | Swift, Zig | Include | See chat |
| `deinit` | Runs when a value goes away: frees memory, closes files | Swift, C++ | Include | The counterpart of `init` |
| `new` | Put a value on the heap and get an owned box back | C++, Java | Include | Safe: the box is owned |
| `delete` | Free a boxed value now, instead of waiting for the end of its scope | C++ | Include | Safe: it consumes the box |
| `this` | Another name for `self` | Java, C++, JS | Leave out | Keep `self` |

## Variables

| Word | Meaning | Seen in | Decision | Why |
|---|---|---|---|---|
| `let` | Reserved. Now also the read-only access mode | Rust, Swift, Lisp, ML | Mode | `const` and `var` cover declarations |
| `val` / `mut` | Other spellings for `const` / `var` | Scala, Kotlin / Rust | Rename option | |
| `ref` | A reference stored as a value | C#, Rust | Leave out | References are temporary under value semantics |
| `null` / `nil` / `undefined` | "No value here" | everywhere | `none` instead | Absence is visible in the type |
| `void` | A function that gives back nothing | C, Java | Include | |
| `never` | A function that never returns | TS, Rust | Include | |
| `typeof` `sizeof` `alignof` | Ask for a type, its size, its alignment | JS, C | Include | Probably compile-time functions |
| `volatile` `atomic` | Memory that hardware or other threads can change | C, C++ | Include | Phase 3 |

## Other control flow

| Word | Meaning | Seen in | Decision | Why |
|---|---|---|---|---|
| `then` / `do` / `end` | Block delimiters without braces | Pascal, Lua, Ruby | Leave out | Braces |
| `goto` | Jump to a label | C | Leave out | Hard to read and optimize |
| `try` `catch` `throw` `finally` | Exceptions | Java, JS, Python | Leave out | Hidden control flow. `result` and `?` instead |
| `fallthrough` | Continue into the next `case` | C, Swift | Leave out | No silent fallthrough |
| `yield` | Produce values one at a time from a function | Python, JS | Include | Phase 3 |
| `with` | Scoped resource handling | Python | Leave out | `deinit` and `defer` do this |

## Safety, attributes and the edges

| Working | Meaning | Seen in | Phase | Your rename |
|---|---|---|---|---|
| `unsafe` | A block where the compiler's safety checks are switched off | Rust, C# | 3 | |
| `async` / `await` | Concurrency without blocking a thread | JS, Rust, C# | 3 | |
| `test` | A built-in test block that only compiles in test mode | Zig | 3 | |
| `assert` | Crash if this condition is false | C, Python | 1 | |
| `panic` | Crash now with a message | Rust, Go | 1 | |
| `unreachable` | Tell the compiler this point can never be reached | Rust, Zig | 3 | |
| `todo` | Placeholder that crashes if reached | Rust | 2 | |
| `unchecked` | Skip bounds or overflow checks for speed, only inside `unsafe` | Rust (`get_unchecked`) | 3 | |
| `wrapping` / `saturating` / `checked` | Arithmetic that wraps around / stops at the limit / returns `none` on overflow. Library functions or operators | Rust | 2 | |
| `when` | Compile this only for certain targets or modes (debug, release, OS, CPU) | Rust (`cfg`) | 3 | |
| `deprecated` | Mark something as going away | Java, Rust | 3 | |
| `@name` | Attribute syntax that attaches notes to a declaration, such as `@inline` | Java, Python decorators | 3 | |
| `must_use` | Warn if a result is ignored | Rust | 3 | |
| `abi` / `layout` | Say how a type or function is arranged in memory for outside code | Zig, Rust | 3 | |

## Low level and concurrency

| Working | Meaning | Seen in | Phase | Your rename |
|---|---|---|---|---|
| `spawn` / `go` | Start a task that runs at the same time | Go, Rust | 3 | |
| `chan` | A channel: a pipe that one task puts values into and another takes values out of | Go | 3 | |
| `select` | Wait on several channels at once and act on whichever is ready first | Go | 3 | |
| `asm` | Inline assembly | C, Rust | 3 | |
| `inline` / `noinline` | Hints to the optimizer | C, Rust | 3 | |
| `align` / `packed` | Control memory layout | C, Zig | 3 | |

## Type names

Decided: lowercase, and sized integers from 8 to 64 bits. `int` is always 64 bits.

| Name | Meaning | Phase |
|---|---|---|
| `int` | Whole number, signed, 64 bits | 1 |
| `int8` `int16` `int32` `int64` | Signed whole numbers of an exact size | 2 |
| `uint` `uint8` `uint16` `uint32` `uint64` | Unsigned whole numbers | 2 |
| `byte` | Another name for `uint8`, for raw data | 2 |
| `float` | Decimal number, 64-bit | 1 |
| `float32` `float64` | Decimal numbers of an exact size | 2 |
| `bool` | `true` or `false` | 1 |
| `char` | One character (a Unicode scalar) | 1 |
| `string` | Text, owned, UTF-8 | 1 |
| `str` | A read-only view of text inside another string (a "slice") | 2 |
| `[T]` | A growable list of `T` | 2 |
| `[T; N]` | A fixed-size array of exactly `N` items of `T` | 2 |
| `(A, B)` | A tuple | 2 |
| `map<K, V>` | Key to value lookup | 2 |
| `set<T>` | A collection of unique items | 2 |
| `range` | What `0..10` is | 2 |
| `option<T>` | Either a `T` or `none` | 2 |
| `result<T, E>` | Either a success `T` or an error `E` | 2 |
| `box<T>` | An owned value on the heap (what `new` gives you) | 2 |
| `rc<T>` | Optional shared ownership with counting, for the rare case you need it | 3 |
| `ptr<T>` | A raw pointer, only inside `unsafe` | 3 |
| `fn(A) -> B` | The type of a function | 2 |
| `void` / `never` | Returns nothing / never returns | 2 |
| `error` | The interface that all error types provide | 2 |

---

## Still open

1. **A new name for the compile-time keyword.** `bake`, `meta`, `pre`, `eval`, `build`.
2. **`module`, `mod`, `pkg`, `unit`?** `pkg` fits best as the name for a distributable bundle of modules, later.
3. **Inheritance path:** staged (interfaces with default methods and composition now, `class` later) or `class` from the start.
4. **The inheritance keyword:** `extends`, `inherit`, `from`, `:`.
5. **The three-part `for`** is in, and so is `++` and `--` (open).
6. **Is `null` meant to exist at all,** or only `none`?
