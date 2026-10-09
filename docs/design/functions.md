# Functions (draft 1)

One idea at a time, like the basics page. Syntax is a sketch to react to.

**Already decided and used here:** `fn`, `->` for return types, lowercase types,
`const` / `var`, `#` in strings, `let` / `inout` / `sink` access modes with
`inout` repeated at the call site, `<T>` generics, `result` with `?`,
`none` instead of null, `~>` closures with `it`, `pub` / `priv` (private by default).

## 1. Defining and calling

```
fn greet(name: string) {
    print("Hello, #name")
}

greet("Dennis")             // Hello, Dennis
```

A function has a name, parameters with their types after the names, and a block.
There is no return type, so it gives back nothing.

## 2. Giving a value back

```
fn add(a: int, b: int) -> int {
    a + b                   // the last expression is the result
}

fn first_even(xs: [int]) -> option<int> {
    for x of xs {
        if x % 2 == 0 { return x }    // `return` leaves early
    }
    none
}
```

The last expression of the block is the value. `return` is only for leaving early.

## 3. A short form for one-liners

```
fn double(x: int) -> int = x * 2
```

Same function, no braces. See question F1.

## 4. How a parameter is used (access modes)

Read-only is the silent default. The other two are written out.

```
fn total(xs: [int]) -> int { ... }            // read-only, nothing copied
fn clear(inout xs: [int]) { ... }             // may change the caller's list
fn finish(sink xs: [int]) -> int { ... }      // takes the list away

var scores = [3, 1, 2]
print(total(scores))
clear(inout scores)                           // the caller repeats `inout`
```

(`let` can be written in front of a read-only parameter, and the formatter removes it.)

## 5. Default values and named arguments

```
fn greet(name: string, greeting: string = "Hello") {
    print("#greeting, #name")
}

greet("Dennis")                       // Hello, Dennis
greet("Dennis", greeting: "Hi")       // Hi, Dennis
```

A parameter with a default can be left out. At the call site, `name: value` says
which parameter you mean. See question F2.

## 6. Giving back more than one value

```
fn divmod(a: int, b: int) -> (int, int) {
    (a / b, a % b)
}

const (q, r) = divmod(7, 2)           // q is 3, r is 1
```

## 7. When something can fail

```
fn parse_int(s: string) -> result<int, parse_error> {
    ...
}

fn double_input(s: string) -> result<int, parse_error> {
    const n = parse_int(s)?           // on an error, return it right now
    ok(n * 2)
}
```

There are no exceptions. A function that can fail says so in its type, and `?`
passes the failure upward. `ok(...)` and `err(...)` make the two kinds of result.

## 8. When a value may be missing

```
fn find(xs: [string], name: string) -> option<int> { ... }

const index = find(names, "Dennis") ?? -1      // use -1 if nothing was found
```

A plain `int` converts to `option<int>` by itself, so there is no `some(...)`.

## 9. Generic functions

```
fn max<T: ordered>(a: T, b: T) -> T {
    a > b ? a : b
}

max(3, 7)              // 7
max("ant", "bee")      // "bee"
```

`T: ordered` means "any type that provides the `ordered` interface". The compiler
makes a specialised copy for each type that is used, so there is no runtime cost.

## 10. Functions as values and closures

```
fn apply_twice(f: fn(int) -> int, x: int) -> int {
    f(f(x))
}

apply_twice(x ~> x + 3, 10)           // 16
numbers.map(~> it * 2)

var total = 0
numbers.each([inout total] x ~> total += x)
```

`fn(int) -> int` is the type of a function. A closure copies or moves what it uses,
unless an access mode in brackets says otherwise.

## 11. Methods

```
struct counter {
    count: int

    fn show(self) { print("Count: #(self.count)") }
    fn bump(inout self) { self.count++ }
    fn close(sink self) -> int { self.count }
}

var c = counter { count: 0 }
c.bump()               // how should this look? See question F3
c.show()
```

A method is a function inside a `struct`, `enum` or `obj` whose first parameter
is `self`, with the same three modes.

## 12. Order, privacy and the entry point

- Functions can be used before they are written. Order inside a module does not matter.
- Private to the module by default. `pub` makes one visible outside.
- A program starts at `fn main()`. See question F6.

## What is already our own

- Access modes on every parameter, with `inout` visible at the call site.
- No exceptions: failure and absence are types (`result`, `option`), passed on with `?` and `??`.
- `~>` closures with the implicit `it`.
- A last-expression result and a `=` shorthand that keep functions short in tokens.

## Later

Variadic parameters, tail calls, `meta fn` (compile-time functions), inline hints,
async functions, and function overloading beyond what generics give.

---

## Questions for you

**[F1] The one-line form.** `fn double(x: int) -> int = x * 2`. Keep it? Alternatives:
`=> x * 2`, or none (always braces).

**[F2] Named arguments.** Options:
- A. Optional: `greet("Dennis", greeting: "Hi")`. Positional always works, names are
  allowed on any parameter, and you can reorder named ones (my recommendation).
- B. Required for everything after the first (Swift-like), so calls always read well.
- C. None. Use a struct when you need names.

**[F3] Calling a method that changes `self`.** Options:
- A. No marker: `c.bump()`. Shortest, but the mutation is hidden at the call site.
- B. Repeat `inout`: `inout c.bump()`. Consistent with the `inout a` rule for arguments.
- C. A `!` on the name: `c.bump!()` (Ruby does this). Short and visible.
- Same question for `sink self`: should `c.close()` show that `c` is consumed?

**[F4] Overloading.** Two functions with the same name but different parameter types.
Allowed, or not? My recommendation is no: defaults, generics and interfaces cover it,
and the compiler stays simpler and faster.

**[F5] Casing of result constructors.** `ok(5)` and `err(e)` (lowercase, like `none`)
or `Ok(5)` and `Err(e)`?

**[F6] The entry point.** `fn main()`, or `fn main(args: [string])`, or
`fn main() -> result<void, error>` so `?` works at the top level? I would allow all three.

**[F7] Shorthand types.** `int?` for `option<int>`, and `int ! parse_error` for
`result<int, parse_error>`? They save tokens. Cost: another meaning for `?`
(this time in types).

**[F8] Generic bounds.** `fn max<T: ordered>(a: T, b: T)` (shown above) or a `where`
clause after the signature for longer ones. I would allow both.
