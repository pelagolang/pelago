# List patterns (draft 2)

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
- **Text supports both** literal cases and list patterns (LP7). Over a `string` or `str` the elements are
  `char`s and the rest is a `str` view: `case ['a', ...rest]:` or `case [c, ...]:`. Literals stay
  the simple way to test a whole value (`case "add":`). Lengths are counted in characters, so a length
  test walks the text once; matching from the front never decodes more than the pattern needs.

## 5. Closing the builder question (B5)

Builders needed a way to skip items that don't match. `is` already binds names, so no new
syntax is needed. A filter written with `is` makes its names available to the result:

```
const radii = [s of shapes if s is .circle(r) ~> r]                  // only circles
const firsts = [x of rows if x is [first, ...] ~> first]             // rows that are not empty
const values = [o of opts if o is some(v) ~> v * 2]                  // some options only
```

(The present case of an `option` is `some(v)`; see the decisions below.)

## 6. Why not other designs

- **`[first, ..rest]` (Rust):** `..` already means "ignore other fields" in struct patterns, and
  `...` is the spread everyone sees in lists.
- **`first :: rest` (ML / Haskell):** cons patterns assume linked lists. Ours are arrays, where
  "the rest" is a view, and the middle and end cases (`[...init, last]`) are as cheap as the front.
- **A separate `match`-for-lists:** one `switch` for everything was the decision.

## 7. Left for later (revisit after the checker exists)

- **Remember for later versions: `sink` in list patterns.** Consuming list patterns (`switch sink xs { case [a, ...rest]: ... }`, where `a` and `rest` become
  owned). Possible once `sink` on an owning list can hand back pieces safely; needs the checker first.
- **Patterns on maps and sets.** Look-ups stay `map.get(k)` returning an `option`, matched normally.
- **Patterns on iterators**, which cannot be stored and have no length.

## Decisions from draft 1

- **LP1** `...` is the rest marker (bare to ignore, `...name` to bind), at most one per pattern.
- **LP2** `rest` is a temporary slice view; `copy rest` makes an owned list.
- **LP3** Access modes lead the binding: `[inout first, ...inout rest]`.
- **LP4** `sink` on list-pattern elements is not allowed in v1. Kept on the list of things to revisit (section 7).
- **LP5** Fixed arrays can be destructured in plain declarations; lists use `guard ... is` or `if ... is`.
- **LP6** (a) `if x is pattern` in a builder brings its names into the result (closes B5). (b) The present case of an
  `option` is matched as `some(v)`, a pattern-only word like `ok` and `err`; values are still written bare.
- **LP7** Text gets both: literal cases and list patterns over `char`s with a `str` rest.
- **LP8** The rest marker may sit in the middle.
