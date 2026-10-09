# switch (draft 2)

One construct for choosing by value and by shape. It replaces `match`.

**Already decided:** `switch` / `case` / `default`, several values in one case (`case 1, 2, 3:`),
no fallthrough, access modes (`let` / `inout` / `sink`) wherever a name is bound, `is` to test a case
or a type, `if` as a guard word, `none` / `ok(...)` / `err(...)`, leading-dot enum cases (`.circle`).

## 1. Choosing by value

```
switch day {
    case 1: print("Monday")
    case 2: print("Tuesday")
    case 6, 7: print("Weekend")
    default: print("Another day")
}
```

Each case runs on its own. There is no fallthrough, so there is no `break` to forget.

## 2. An arm is everything up to the next case

```
switch command {
    case "start":
        load_config()
        run()
    case "stop":
        shutdown()
    default:
        print("unknown command")
}
```

An arm holds statements, one line or many, until the next `case`, `default` or the closing brace.
No braces are needed, and the arms can also share a line: `switch x { case 1: "a" case 2: "b" default: "c" }`.

`break` inside a `switch` leaves the enclosing **loop**, never the switch. (In C it leaves the
switch, which surprises everyone; here an arm already ends by itself.)

## 3. As a value

The last expression of an arm is its value, so a switch can be used anywhere a value can.

```
const name = switch day {
    case 1: "Monday"
    case 6, 7: "Weekend"
    default: "Another day"
}
```

All arms must give the same type. For two or three outcomes, the chained ternary is shorter;
use `switch` when there are more.

## 4. Patterns

A case holds a *pattern*: a shape the value is compared against.

| Pattern | Matches | Example |
|---|---|---|
| a literal | that value | `case 3:`, `case "start":`, `case 'a':`, `case true:` |
| a range | any value inside | `case 0..10:`, `case 90..=100:` |
| `_` | anything, ignoring it | `case (_, 0):` |
| a name | anything, and **binds** it | `case n:`, `case (x, 0):` |
| a tuple | a tuple, piece by piece | `case (0, 0):` |
| an enum case | that case, and its payload | `case .circle(r):` |
| `none`, `ok(p)`, `err(p)` | the library cases | `case ok(n):` |

```
switch (x, y) {
    case (0, 0): "origin"
    case (_, 0): "on the x axis"
    case (0, _): "on the y axis"
    case (a, b): "at #a, #b"
}
```

**A bare name always binds.** It never compares. That is how the classic bug disappears where
`case empty:` silently matches everything because `empty` was a typo for the enum case. Enum
cases are written with a leading dot (`.empty`) or the type name (`shape.empty`), and the
library cases (`none`, `ok`, `err`) are the only ones allowed bare. To compare against an
existing variable, use a guard (below).

## 5. Enums and their payloads

```
enum shape {
    circle(radius: float)
    rect(w: float, h: float)
    empty
}

fn area(s: shape) -> float {
    switch s {
        case .circle(r): PI * r * r
        case .rect(w, h): w * h
        case .empty: 0.0
    }
}
```

The compiler checks that every case is handled, so adding a case to the enum makes every
`switch` that forgot it a compile error. `default` is only required when the cases can't be
listed (numbers, strings).

Nested patterns work: `case err(.empty): 0` and `case ok(.circle(r)): r`.

## 6. Access modes in patterns

A bound name is read-only unless you say otherwise, the same three words as everywhere else.

```
switch s {
    case .rect(inout w, h): w += 1.0        // change the width inside s, in place
    case .circle(sink r): consume(r)        // take the payload out
    case .circle(r): print(r)               // read-only (the default)
}
```

## 7. Guards

`if` adds a condition to a case.

```
switch score {
    case n if n > 100: "off the scale"
    case 90..=100: "A"
    case n if n < 0: "invalid"
    default: "keep going"
}
```

(A guard is how you compare with an existing variable: `case n if n == limit:`.)

## 8. Testing without a switch

`is` runs the same pattern test in an ordinary condition, and the names it binds are usable
inside the block.

```
if shape is .circle(r) {
    print("radius #r")
}

guard shape is .circle(r) else { return }
print(r)                                     // r is available after the guard

if value is none { ... }
```

This is also the answer to "if let": no extra keyword.

## 9. Objects

An `obj` variable can hold a subtype. `is` with a name tests and converts.

```
switch pet {
    case d is dog: d.fetch()
    case c is cat: c.purr()
    default: pet.speak()
}

if pet is dog { ... }                        // test only
const maybe_dog = pet to? dog                // option<dog>
```

## 10. Struct patterns

```
switch p {
    case point { x: 0, y: 0 }: "origin"
    case point { x: 0, y }: "on the y axis at #y"
    case point { x, .. }: "x is #x"
}
```

- `field: pattern` matches one field. A lone name binds that field under its own name.
- `..` means "ignore the other fields". It is **required** when a field is left out, so a pattern
  never silently ignores a field that was added later without saying so.
- Enum payloads with labels can be matched by label: `case .rect(w: width)`, or by position.
- Access modes work as everywhere: `point { inout x, y }`.
- The same shapes destructure in declarations: `const point { x, y } = p`, `const (a, b) = pair`.

## 11. Alternatives that bind names

```
switch s {
    case .circle(r), .square(r): print("one measure: #r")
    case .rect(w, h): print("#w by #h")
}
```

Allowed when **every alternative binds the same names, with the same types and the same access
modes**. Otherwise the compiler says which name is missing. (The first version of the parser
rejects any binding in a comma list, as agreed; this is the rule that switches it on.)

## 12. Open ranges

```
switch n {
    case ..0: "negative"          // below 0
    case 0: "zero"
    case 1..=9: "a digit"
    case 10..: "ten or more"      // 10 and up
}
```

The full set: `a..b`, `a..=b`, `a..`, `..b`, `..=b`, and `..` (everything). They work outside
patterns too: `xs[2..]`, `xs[..3]`, `x in 100..`.

## 13. Naming the whole value while matching

`name is pattern` binds the whole value and also requires it to match.

```
switch n {
    case big is 100..=999: "three digits (#big)"
}

switch s {
    case whole is .circle(r): grow(whole, r)
}
```

This is the same `is` as `case d is dog:`, so a type test is just one more pattern, and no
`@` symbol is needed.

## Later still

Patterns over lists (`[first, ..rest]`): under value semantics `rest` must be a view of the list,
which depends on how collections are designed. Revisit with collections.

## What is already our own

- A bare name never compares, so the silent match-everything bug is gone.
- `break` never means "leave the switch".
- One construct covers value choice, enum matching and object type tests.
- `is` brings the same patterns to conditions without a second keyword.

---

## Decisions from draft 1

- **S1** An arm is all statements up to the next `case`, `default` or the closing brace.
- **S2** A bare name always binds. Enum cases take a leading dot or the type name.
- **S3** `break` leaves the enclosing loop, never the switch.
- **S4** `default:` is the spelling. `case _:` also works and the formatter writes `default:`.
- **S5** `is` carries type patterns and tests (`case d is dog:`, `x is .circle(r)`).
- **S6** No names bound inside comma-separated alternatives in the first version (rule for later in section 11).
- **S7** `0..10` excludes the end, `0..=10` includes it, in patterns too.

## Quick confirms for sections 10 to 13

- **N1** `..` is required in a struct pattern that leaves fields out.
- **N2** `name is pattern` instead of an `@` symbol.
- **N3** The open range forms above.
