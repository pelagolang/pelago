# List patterns (draft 1)

Patterns that look inside a list, array or slice. They plug into everything in `switch.md`: `switch`
arms, `is` in conditions, `guard ... else`, guards with `if`, alternatives, and `name is pattern`.

## 1. The shapes

```
switch xs {
    case []:                       "empty"
    case [x]:                      "one: #x"
    case [a, b]:                   "two: #a and #b"
    case [first, ...rest]:         "#first, then #(rest.len()) more"
}
```

| Pattern | Matches |
|---|---|
| `[]` | a list with no elements |
| `[a, b]` | exactly two elements, bound to `a` and `b` |
| `[a, ...rest]` | one or more; `rest` is everything after `a` (possibly empty) |
| `[...init, last]` | one or more; `init` is everything before `last` |
| `[first, ...mid, last]` | two or more, with the middle bound to `mid` |
| `[first, ...]` | one or more; the rest is ignored (a bare `...`) |
| `[0, x]`, `[.circle(r), ...]` | elements can be any pattern: literals, ranges, enum cases, nested lists |

- **The rest marker is `...`**, the same symbol as spread in `[0, ...xs, 99]`: in an expression it
  spreads items out, in a pattern it gathers items in. Struct patterns keep `..` for "ignore the
  other fields"; the two differ because a list's rest has a length and usually a name.
- At most one rest marker per pattern.
- Works on `[T]`, `[T; N]` and slice views. With a fixed array the length is known, so `[a, b]` on a
  `[int; 2]` always matches and the compiler knows it.

## 2. The rest is a view

`rest` is a **slice view** (collections.md section 2), not a new list. So it costs nothing to make
(a pointer and a length) and, like every view, it lasts as long as the arm and cannot be stored or
returned. To keep one: `copy rest`.

```
fn sum(xs: [int]) -> int {
    switch xs {
        case []:             0
        case [h, ...t]:      h + sum(t)          // t is a view; passing it down is fine
    }
}

fn count(xs: [int], acc: int) -> int {
    switch xs {
        case []:             acc
        case [_, ...t]:      become count(t, acc + 1)    // guaranteed tail call
    }
}
```

## 3. Access modes

Modes work on bindings as everywhere. The mode goes in front of the binding it applies to:

```
switch inout xs {
    case [inout first, ...inout rest]:
        first += 1                  // changes xs[0] in place
        rest.reverse!()             // changes the rest in place
    case []: ...
}
```

- Default is read-only (`let`). `inout` needs the scrutinee to be `inout` too.
- **`sink` is not allowed on elements**: taking one element out of a list would leave a hole. To take
  a whole list apart, use `pop`/`remove`-style methods, or a future `sink` list pattern (section 7).

## 4. Where they work

```
if args is [cmd, ...rest] { run(cmd, rest) }

guard args is [path] else { return err(usage()) }       // path is usable after the guard

switch line.split(" ").to_list() {
    case ["add", name]:            add!(name)
    case ["remove", name]:         remove!(name)
    case ["list"]:                 list()
    case ["rename", old, new]:     rename!(old, new)
    case [cmd, ...]:               print("unknown command #cmd")
    case []:                       print("empty line")
}
```

- **Exhaustiveness:** the compiler tracks lengths. `[]` plus `[_, ...]` is complete; so is `[]`,
  `[_]`, `[_, _, ...]`. Missing lengths are named in the error.
- **Declarations must not fail.** `const [a, b] = xs` is accepted only when `xs` is a fixed array
  with that length (`[int; 2]`). For a list, use `guard xs is [a, b] else { ... }` or `if xs is`.
- Alternatives work when they bind the same names: `case [x], [x, _]:`.
- `name is pattern` works: `case whole is [first, ...]:` keeps the list and its first element.
- Strings are not lists: no `[c, ...]` over text. Literal patterns (`case "add":`) work, and
  prefix tests stay as methods (`s.starts_with("add ")`).

## 5. Closing the builder question (B5)

Builders needed a way to skip items that don't match. `is` already binds names, so no new
syntax is needed. A filter written with `is` makes its names available to the result:

```
const radii = [s of shapes if s is .circle(r) ~> r]                  // only circles
const firsts = [x of rows if x is [first, ...] ~> first]             // rows that are not empty
const values = [o of opts if o is some(v) ~> v * 2]                  // some options only
```

(The docs name `none`, `ok(p)` and `err(p)` as library patterns but never say how the present case of an
`option` is written. `some(v)` above is a placeholder; see LP6.)

## 6. Why not other designs

- **`[first, ..rest]` (Rust):** `..` already means "ignore other fields" in struct patterns, and
  `...` is the spread everyone sees in lists.
- **`first :: rest` (ML / Haskell):** cons patterns assume linked lists. Ours are arrays, where
  "the rest" is a view, and the middle and end cases (`[...init, last]`) are as cheap as the front.
- **A separate `match`-for-lists:** one `switch` for everything was the decision.

## 7. Left for later

- **Consuming list patterns** (`switch sink xs { case [a, ...rest]: ... }`, where `a` and `rest` become
  owned). Possible once `sink` on an owning list can hand back pieces safely; needs the checker first.
- **Patterns on maps and sets.** Look-ups stay `map.get(k)` returning an `option`, matched normally.
- **Patterns on iterators**, which cannot be stored and have no length.

## Questions

**LP1.** The rest marker is `...` (bare to ignore, `...name` to bind). Keep it?
**LP2.** `rest` is a temporary view, `copy rest` to own it?
**LP3.** Modes lead the binding: `[inout first, ...inout rest]`. Keep that order?
**LP4.** `sink` on elements is not allowed in v1?
**LP5.** Fixed arrays may be destructured in plain declarations; lists must use `guard` / `if is`?
**LP6.** (a) B5 is closed by `if x is pattern` bringing its names into the builder's result, with no new syntax. Fine?
(b) A gap I found while writing this: `switch.md` has `none` as a pattern but no name for the *present* case of an `option`.
I suggest `some(v)`, mirroring `ok(v)` and `err(e)`; then `some` is a pattern-only word, not a constructor (you still write the bare value).
Alternatives: a bare `v` binding after `case none` (works in `switch`, not in `is`), or `.some(v)` like an enum case.

**LP7.** No list patterns over strings (literals only)?
**LP8.** Rest in the middle (`[first, ...mid, last]`) allowed? It costs a little in exhaustiveness code, nothing at run time.
