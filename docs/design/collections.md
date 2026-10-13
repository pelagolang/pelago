# Collections and strings (draft 1)

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
- No negative indexes: `xs[-1]` is an error. Use `xs.last()`, which is an `option`.
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
- There are no list comprehensions: `map`, `filter` and `~>` already do that job.

## 9. Building with spread

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

## Questions for you

**[C1] Index rule.** `xs[i]` and `m[key]` stop the program when missing, `.get` returns an `option` (my recommendation),
or should map lookup return an `option` by default?

**[C2] Negative indexes.** Not allowed (use `.last()`). Keep?

**[C3] Map order.** Insertion order by default. Keep?

**[C4] Strings.** The static-data representation with copy on first change, and no `s[i]`. Right?

**[C5] `+` on strings.** Allowed alongside interpolation?

**[C6] Set literals.** `set { 1, 2, 3 }`, matching `map { ... }`. Fine?

**[C7] Spread in literals.** `[0, ...xs]` and `map { ...d, "k": 1 }`. Keep?

**[C8] Method naming.** `sort!()` changes in place, `sorted()` returns a new list. Keep?

**[C9] No comprehensions.** Agreed?
