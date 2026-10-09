# Basics: variables and types (draft 1)

Written the way a first Python lesson would go, one idea at a time. Syntax is a
sketch to react to, not a final decision.

## 1. A variable is a name for a value

```
var score = 10
const name = "Dennis"
```

- `var` makes a variable you can change later.
- `const` makes one you cannot. Because of value semantics, "cannot change" really
  means it: nothing reachable through that name can change either.
- Names are written before values. No type is written, because the compiler works it out.

## 2. Changing a variable

```
var score = 10
score = 15          // fine
score += 5          // score is now 20

const pi = 3.14
pi = 3.2            // error: pi is const
```

## 3. Types

Every value has a type, and a variable keeps its type forever. This is the big
difference from Python.

```
var score = 10
score = "ten"       // error: score is an Int, "ten" is a String
```

The basic types:

| Type | Holds | Example |
|---|---|---|
| `Int` | whole numbers | `42`, `-7` |
| `Float` | decimal numbers | `3.14` |
| `Bool` | true or false | `true` |
| `Char` | one character | `'a'` |
| `String` | text | `"hello"` |

## 4. Writing the type yourself

The type goes after the name, like TS. You only need it when the compiler can't
work it out, or when you want to be explicit.

```
var score: Int = 10
const ratio: Float = 0.5
var count: Int            // error: a variable must be given a value
```

A variable must always be given a value when it is created. There is no
uninitialized variable and no null, which removes a whole class of bugs.

## 5. Printing and comments

```
print("Hello")
print(score)
// this is a comment
```

## 6. Converting between types

Conversions are always written out, never silent.

```
const whole = 7
const decimal = Float(whole)     // 7.0
const back = Int(3.9)            // 3 (the decimal part is dropped)
```

---

## Questions for you

**[B1] Variable names:** `snake_case` (Python, Rust) or `camelCase` (JS, TS, Swift)?
Type names stay `Capitalized` either way.

**[B2] Size of `Int`:** a 64-bit whole number everywhere (simple, like Go and Swift),
or 32-bit by default (like Rust's `i32`)? Exact sizes (`Int8` ... `Int64`) exist either way.

**[B3] Type names:** `Int`, `Float`, `Bool` capitalized (Swift), or lowercase `int`,
`float`, `bool` (Python, TS), or Rust's `i32`, `f64`?

**[B4] Conversions:** `Float(whole)` like a function call (Python, Swift), or
`whole as Float` (Rust, TS)?

**[B5] Strings with values inside:** how to write "Score: 10" using a variable.
Options: `"Score: ${score}"` (JS), `f"Score: {score}"` (Python), or `"Score: \(score)"` (Swift).
