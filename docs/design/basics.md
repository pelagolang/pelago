# Basics: variables and types (draft 2)

Written the way a first Python lesson would go, one idea at a time. Syntax is a
sketch to react to, not a final decision.

**Decided so far:** `fn` for functions, `var` and `const`, lowercase type names,
sized integers from 8 to 64 bits, `<T>` generics, newlines end statements.

## 1. A variable is a name for a value

```
var score = 10
const name = "Dennis"
```

- `var` makes a variable you can change later.
- `const` makes one you cannot. Because of value semantics, "cannot change" really
  means it: nothing reachable through that name can change either.
- The name comes first. No type is written, because the compiler works it out.

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
score = "ten"       // error: score is an int, "ten" is a string
```

| Type | Holds | Example |
|---|---|---|
| `int` | whole numbers | `42`, `-7` |
| `float` | decimal numbers | `3.14` |
| `bool` | true or false | `true` |
| `char` | one character | `'a'` |
| `string` | text | `"hello"` |

Exact sizes for when you care about memory:

| Signed | Unsigned | Range of the signed one |
|---|---|---|
| `int8` | `uint8` | -128 to 127 |
| `int16` | `uint16` | about -32 thousand to 32 thousand |
| `int32` | `uint32` | about -2.1 billion to 2.1 billion |
| `int64` | `uint64` | about -9.2 quintillion to 9.2 quintillion |

and `float32`, `float64` for decimals.

## 4. Writing the type yourself

The type goes after the name, like TS. You only need it when the compiler can't
work it out, or when you want to be explicit.

```
var score: int = 10
const ratio: float = 0.5
var small: int8 = 100
var count: int            // error: a variable must be given a value
```

A variable must always be given a value when it is created. There is no
uninitialized variable and no null, which removes a whole class of bugs.

## 5. Printing and comments

```
print("Hello")
print(score)
/// this is a comment
```

## 6. Converting between types (proposal, custom)

Conversions are always written out, never silent. Proposal: a plain word, `to`.

```
const whole = 7
const decimal = whole to float      // 7.0
const text = whole to string        // "7"
```

The compiler only allows `to` when nothing can be lost. Conversions that can
lose information or fail will need a different, visible spelling. We work that
out when we reach errors.

## 7. Strings with values inside (decided: our own `#` marker)

A `#` followed by a name puts that value into the text. For anything bigger than
a name, wrap it in brackets.

```
print("Score: #score")              // Score: 10
print("Next: #(score + 1)")         // Next: 11
print("A literal hash: \#")         // A literal hash: #
```

The expression is checked when the program is compiled, so a mistake is an
error before the program runs. Formatting control (decimals, width), multi-line
strings and raw strings come later and don't change this marker.

---

## What is already our own

Syntax is the cheapest place to be different, so the list below is mostly about
what the language *does*. Everything here is a deliberate choice, not a copy.

- A TS-looking language with **no GC and no references you store**, only value semantics.
- **`inout` repeated at the call site**, so you see every mutation where it happens.
- **`const` that is truly immutable**, all the way down.
- **No null.** Absence is a type (`option<T>`), never a surprise.
- **Conversions written as `to`**, and only when nothing can be lost (proposal).
- **Strings that interpolate with `#`**, a marker we haven't seen elsewhere.
- **One `switch`/`case` that is also pattern matching.**
- **Token efficiency as a design goal**, measured against real tokenizers.

## Decisions from draft 2

- **[B1]** The compiler accepts any naming style. The standard library and formatter pick one.
- **[B2]** `int` is always exactly 64 bits.
- **[B3]** Type names are lowercase. (Open: your own types too?)
- **[B4]** Conversions are written `x to float`.
- **[B8]** The house naming style is `camelCase` for functions, methods, variables and fields (`isEmpty`,
  `parseInt`, `wrappingAdd`); types keep lowercase (B3) and constants may be anything. The standard
  library, docs and examples use it. B1 still holds: any style compiles, and a name that is not found is
  tried once in the other style (`is_empty` finds `isEmpty`, `isEmpty` finds `is_empty`; an exact match
  always wins). So snake_case code keeps working and calls the camelCase library unchanged. The `rt_*`
  runtime hooks in `core.pel` are internal and keep their names. The formatter still renames nothing.

- **[B5]** String interpolation uses `#name` and `#(expression)`.
- **[B6]** Both lowercase and capitalized names are allowed for your own types. The compiler
  accepts either, like variable names (B1). The formatter and standard library use a capital
  for types you define, so they stand out. A type and a value in the same scope can't share a name.

## Still open

**[B7] The `let` keyword.** Currently reserved and unused. Leading candidate for later:
the read-only member of the access-mode trio (`let` / `inout` / `sink`), possibly optional
because it is the silent default. Places the trio could appear: function parameters
(including `self`), loop variables (`for inout x in xs` to change elements in place),
pattern bindings in `switch`, and closure captures. Decide when we reach functions.
