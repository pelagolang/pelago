# Collections and strings (draft 4)

One idea at a time. Syntax is a sketch to react to.

**Already decided and used here:** `[T]` list, `[T; N]` fixed array, `(A, B)` tuple, `map<K, V>` with the literal
`map { "a": 1 }` and `{:}`, `set<T>`, `soa<T>`, `string` and `str`, ranges (`0..10`, `0..=10`, `2..`, `..3`),
`for x of items`, `for i, x of items`, `for inout x of items`, `x in 1..10`, `...` spread, `option` for things that
may be missing, `!` on methods that change their receiver, `copy` for owning values, `~>` closures with `it`,
`|>` pipelines, no exceptions, no null.

## 1. Lists

```
var xs = [3, 1, 2]
const names: [string] = []          // an empty list needs its type

xs.push!(4)                         // `!`: this call changes xs
print(xs.len())                     // 4
print(xs[0])                        // 3
xs[1] = 9                           // assign through an index
```

- `xs[i]` must exist. Out of range **stops the program** with a clear message (it is a bug, not a normal case).
  `xs.get(i)` returns `option<T>` when absence is normal.
- **Negative indexes count from the end:** `xs[-1]` is the last element, `xs[-2]` the one before, and `xs[-3..]`
  the last three. `xs.last()` also exists and returns an `option`, for when the list may be empty. An index below
  `-len` stops the program like any other out-of-range index.
  One trap to know about: `xs[i - 1]` with `i` equal to 0 reads the last element instead of stopping, the classic
  Python surprise. A lint can flag computed indexes that can go negative.
- Methods that change the list end in `!`: `push!`, `pop!`, `insert!`, `remove!`, `clear!`, `sort!`, `swap!`.
  Their non-changing partners read like nouns or participles and return a new value: `sorted()`, `reversed()`.
- Lists own their memory and free it when they go away (`deinit`). Passing a list read-only copies nothing.

## 2. Slices are temporary views

```
const head = xs[0..2]               // a view of part of xs
const tail = xs[2..]
sum(xs[1..3])                       // pass a view to a function that reads
sort!(inout xs[1..3])               // or change just that part in place
const own = copy xs[1..3]           // an independent list you can keep
```

A slice is a view that lasts for one statement or one call, like every reference in this language. It cannot be stored
in a struct or returned. `copy` makes a list you own.

## 3. Fixed arrays and tuples

```
var grid: [int; 9] = [0; 9]         // nine zeros: [value; count]
const pair = (3, "three")           // a tuple
const (n, label) = pair
const mixed: (int, string; 4) = ... // your alternating primitive
```

`[T; N]` lives inline (on the stack or inside a struct) and `N` is part of the type, so `meta N: int` works for
generic code over sizes.

## 4. Maps

```
var ages = map { "ann": 31, "bo": 28 }
var counts: map<string, int> = {:}

ages["cy"] = 40                     // insert or replace
print(ages["ann"])                  // 31; a missing key stops the program, like a list index
print(ages.get("zed") ?? 0)         // option: 0 when missing
ages.remove!("bo")                  // returns option<int>
for name, age of ages { print("#name: #age") }
```

- `m[key]` follows the same rule as `xs[i]`: it must exist. `.get(key)` is the forgiving form.
- **Iteration is in insertion order** by default. The same program prints the same output every run, which makes
  tests, builds and bug reports reproducible. A fast unordered `hash_map` can exist later for speed-critical code.
- Keys must provide `eq` and `hash` (`@derive(eq, hash)`).

## 5. Sets

```
var seen = set { 1, 2, 3 }
seen.insert!(4)
print(2 in seen)                    // true
const both = a.intersection(b)      // also union, difference
```

## 6. Strings

```
var s = "héllo"                    // string: owned UTF-8 text
print(s.len())                      // 6: length in bytes
print(s.char_count())               // 5
for ch of s.chars() { ... }        // characters
const head = s[0..3]                // a str view; bytes 0..3; stops if that cuts a character in half
```

- `string` is owned. `str` is a temporary view of text, like a slice; it is what `s[a..b]` gives you and what a
  function takes when it accepts either.
- A string literal costs nothing: `string` can point at the program's own read-only data and copies it the first time it
  is changed. Small strings (up to about 22 bytes) live inline with no heap.
- **No `s[i]`.** An index into UTF-8 text means bytes, characters or visible symbols, and each is wrong half the time.
  Ask for what you want: `s.bytes()`, `s.chars()`, `s.graphemes()`.
- `+` joins strings (`"a" + "b"`), but interpolation `"#a#b"` is usually shorter and clearer.
- Multi-line and raw strings are a lexer topic and come later.

## 7. Ranges

```
for i in 0..10 { }                  // 0 to 9
for i in 0..10 step 2 { }           // 0, 2, 4, 6, 8
const r = 1..=5                     // a value of type range<int>; works in slices, `in` and `switch`
```

## 8. Walking and transforming

```
const total = xs.map(~> it * 2).filter(~> it > 3).sum()
const names2 = users.map(~> it.name) |> sorted
for inout x of xs { x += 1 }        // change every element in place
xs.map!(~> it + 1)                  // the same, as a call
```

- Chains are lazy and the compiler fuses them into one loop (no temporary lists).
- An iterator is a temporary view, so it cannot be stored or returned. Return a list (`.to_list()`) or, later, use a
  generator (`yield`).
- Collect with `.to_list()`, `.to_set()`, `.to_map()`.
- Building a collection from a loop and a filter in one expression has its own shape, the *builder* (section 9).

## 9. Builders: collections from a loop, in our own shape

Python writes `[x * 2 for x in xs if x > 0]` with the result first. Ours starts with **where the items come from**
and ends with **what each one becomes**, joined by the same `~>` ("maps to") that closures use:

```
[x of xs if x > 0 ~> x * 2]
```

Read it left to right: "x of xs, if x is positive, maps to x times two". The source comes first, so names are
known before they are used (good for editors and for readers), and the result sits last where the eye lands.

```
[x of xs if x > 0]                       // no `~>`: keep the item itself (a pure filter)
[i, x of xs ~> "#i: #x"]                 // index and item, like `for i, x of`
[x of xs, y of ys if x != y ~> (x, y)]   // a comma starts a nested loop; `if` filters the one before it
[sink x of xs ~> consume(x)]             // access modes work: take the items out of xs
set { x of xs ~> x % 10 }                // a set of remainders
map { u of users ~> u.name : u.age }     // a map; the result is `key : value`
sum(x of xs if x > 0 ~> x * x)           // in a call, no brackets: a lazy stream, nothing is built
```

- `[ ... ]` builds a list, `set { ... }` a set, `map { ... }` a map: the same literal shapes as before.
- Without brackets, as the only argument of a call, a builder is a **lazy stream** (it is fused into the loop and no list
  is made). That replaces Python's generator expression.
- The clauses run left to right. A name bound earlier can be used later.
- The arrow `~>` at the top of a builder always means "maps to". A closure inside a clause is wrapped in the
  parentheses of its call (`x of xs.map(y ~> y + 1)`), so it never clashes.
- For one simple step the chain is shorter (`xs.map(~> it * 2)`); builders win when there is a filter, two loops or
  an index. Both stay.
- Parsing note: the parser reads `[i, x of xs ...]` as a list until it meets `of`, then re-reads the items before it
  as the pattern, the way JS reads arrow-function parameters.
- No new keyword or symbol: `of`, `if` and `~>` already exist.

## 9b. Building with spread

```
const all = [0, ...xs, 99]
const merged = map { ...defaults, "port": 9090 }
const joined = set { ...a, ...b }
```

## 10. Equality and ordering

`==` compares contents. Lists and strings compare lexicographically with `<`. Maps and sets ignore order when comparing.

## 11. Mutating two elements at once

Exclusive access means you cannot hold `xs[i]` and `xs[j]` as `inout` together. The standard ways around it:
`xs.swap!(i, j)`, and a callback form for the rest: `xs.with_two!(i, j, (a, b) ~> ...)`.

## 12. Memory

Collections take memory from an **allocator**. The default is the global one. An `arena` allocator hands out
memory that is freed all at once, which suits a compiler's syntax tree. Allocator parameters (`list<T, A>`) belong to
a later page; nothing here blocks them.

## What is already our own

- Absence is always visible: `[]` means "must exist", `.get` returns an `option`, and nothing returns a silent null.
- Insertion-ordered maps, so output is reproducible.
- Strings that are cheap to create and refuse a misleading `s[i]`.
- `!` shows every call that changes a collection, and `copy` shows every deep copy.
- `soa<T>` and `[T; N]` give control over layout when it matters.

---

## Decisions from draft 1

- **C1** `xs[i]` and `m[key]` must exist and stop the program when they don't. `.get` returns an `option`.
- **C2** Negative indexes count from the end. `xs.last()` stays as an added option.
- **C3** Maps iterate in insertion order by default.
- **C4** Strings use the static-data representation with copy on first change, and have no `s[i]`.
- **C5** `+` joins strings, alongside interpolation.
- **C6** Set literals are `set { 1, 2, 3 }`.
- **C7** Spread works in list, map and set literals.
- **C8** `sort!()` changes in place, `sorted()` returns a new list.

- **C9** List comprehensions are in, in our own shape: *builders* (section 9).

## Decisions on builders

- **B1** The shape is `[x of xs if x > 0 ~> x * 2]`: source first, result last, joined by `~>`.
- **B2** A builder without brackets as a call's only argument is a lazy stream.
- **B3** A comma starts a nested loop, and `if` filters the loop before it.
- **B4** Maps and sets use `map { ... ~> key : value }` and `set { ... ~> value }`.
- **B5** Skipping items that don't match: write `if x is pattern`; its names are usable in the result (`list-patterns.md`, LP6).
