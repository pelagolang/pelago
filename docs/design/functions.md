# Functions (draft 4)

One idea at a time, like the basics page. Syntax is a sketch to react to.

**Already decided and used here:** `fn`, `->` for return types, lowercase types,
`const` / `var`, `#` in strings, `let` / `inout` / `sink` access modes with
`inout` repeated at the call site, `<T>` generics, `result` with `?`,
`none` instead of null, `~>` closures with `it`, `pub` / `priv` (private by default).

## Decisions from draft 1

- **F1** The one-line form `fn double(x: int) -> int = x * 2` stays.
- **F2** Named arguments are optional. Any parameter can be named, and named ones can be reordered.
- **F3** A method call that changes `self` is marked with `!` on the name: `c.bump!()`. A call that consumes `self` (`sink self`) is marked differently, with `!!`: `c.close!!()`. 
- **F4** No overloading.
- **F5** `ok(5)` and `err(e)`, lowercase.
- **F6** `fn main()`, `fn main(args: [string])` and `fn main() -> result<void, error>` are all allowed.
- **F7** Shorthand types: `int?` for `option<int>`, `int ! parse_error` for `result<int, parse_error>`.
- **F8** Generic bounds can be inline (`<T: ordered>`) or in a `where` clause after the signature.

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

Same function, no braces.

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
which parameter you mean, in any order.

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
Shorthand: `-> int ! parse_error` means `-> result<int, parse_error>`.

## 8. When a value may be missing

```
fn find(xs: [string], name: string) -> option<int> { ... }

const index = find(names, "Dennis") ?? -1      // use -1 if nothing was found
```

A plain `int` converts to `option<int>` by itself, so there is no `some(...)`.
Shorthand: `int?` means `option<int>`.

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
Longer bounds can go in a `where` clause after the signature.

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
c.bump!()              // `!` shows that this call changes c
c.show()               // read-only: no mark
const n = c.close!!()  // `!!` shows c is consumed: it can't be used afterwards
```

A method is a function inside a `struct`, `enum` or `obj` whose first parameter
is `self`, with the same three modes.

## 12. Order, privacy and the entry point

- Functions can be used before they are written. Order inside a module does not matter.
- Private to the module by default. `pub` makes one visible outside.
- A program starts at `fn main()`. It may also be `fn main(args: [string])` or
  `fn main() -> result<void, error>`, so that `?` works at the top level.

## What is already our own

- Access modes on every parameter, with `inout` visible at the call site.
- No exceptions: failure and absence are types (`result`, `option`), passed on with `?` and `??`.
- `~>` closures with the implicit `it`.
- A last-expression result and a `=` shorthand that keep functions short in tokens.

## 13. Any number of arguments (variadic)

```
fn sum(...nums: int) -> int {
    var total = 0
    for n of nums { total += n }
    total
}

sum(1, 2, 3)           // 6
sum()                  // 0
sum(...scores)         // spread an existing list into the call
```

Inside the function, `nums` is a read-only list. The compiler builds it on the stack
at the call, so there is no heap allocation. The variadic parameter comes last, and
`print("score:", 10, true)` is just such a function.

## 14. Recursion and tail calls

A function may call itself. A *tail call* is a call that is the last thing a function
does; the compiler can then reuse the current stack frame and the recursion runs in
constant space, which makes loops-by-recursion safe for the functional style.

```
fn sum_to(n: int, acc: int = 0) -> int {
    if n == 0 { return acc }
    become sum_to(n - 1, acc + n)      // a guaranteed tail call
}
```

`become` means "replace this call with that one". If the compiler cannot do it (for
example a `deinit` still has to run after the call), it is an error, never a silent
stack growth. A plain call stays a plain call, and the optimizer may still turn it into
a tail call when it can. Running out of stack stops the program with a clear message.

## 15. Compile-time code (`meta`)

Three uses of the one keyword:

```
const table = meta build_table()          // run now, while compiling; the result is baked in

meta fn build_table() -> [int; 256] { ... }   // a function that may only run at compile time

fn zeros<meta N: int>() -> [int; N] { ... }   // a value as a generic parameter

meta if target.os == linux {              // include this code only on Linux
    ...
}
```

Compile-time code is ordinary code of the same language, run by an evaluator that
lives inside the compiler. It is never part of the programs we produce, so there
is still no interpreter or runtime in the output. Compile-time code cannot do I/O
at first, and has a step limit so a runaway loop becomes an error, not a hang.
`meta if` would do the job that the `when` keyword was reserved for.

### Built so far (card A-05)

- **`meta if`** (stage M1, `compiler/src/meta.rs`): the condition is evaluated while resolving names, from literals, `target.os`,
  `target.arch`, `target.pointer_bits`, `target.endian`, `target.debug`, bare words such as `linux`, `==`, `!=`, `&&`, `||`, `!` (and
  `<`, `>` on numbers). Only the chosen branch is resolved, checked and lowered; the other is parsed and nothing else, so it may name
  things that exist only on another system. Works for declarations at module level and for statements and values inside functions.
  A condition that is not known at compile time is an error. `Program.target` holds the machine; it defaults to the host.
- **`meta N: int`** (stage M2): a value as a generic parameter on functions and types. `zeros<3>()`, or inferred from an array length
  (`total([1, 2, 3, 4])` makes `N = 4`). Array lengths are type-level terms (`ArrayLen::Term`), `TyKind::Const` carries the number, and each
  value gets its own instance. In the body `N` is an `int` constant. Arithmetic on parameters in types works: `[int; N + 1]`, `N * 2`, `N - 1`
  (`TyKind::ConstOp`, folded once the parameter is known; a known result such as `[int; 4]` unifies with `N + 1` by solving for `N`).
- **`meta expr` and `meta fn`** (stages M3 and M4, `compiler/src/lower/meta.rs`): `const table = meta build_table()` and `meta
  greeting("x")`. The expression becomes a function of its own; everything it reaches is lowered, the module built so far runs in the IR
  interpreter with a step budget, and the value left in memory is read back and written into the program as code (numbers as constants,
  text as static data, lists as an allocation that is filled). Bakeable: numbers, `bool`, `char`, `string`, `str`, arrays, lists, tuples,
  structs, enums, `option`, `result`, `box`, maps and sets (the last two are rebuilt by inserting the entries when the code runs). Not bakeable
  (a clear error): closures, `obj`, `any`. Rules (checker): a `meta
  fn` can be called only inside `meta`; compile-time code cannot `print` (use `meta_note("...")`), call `ext` functions of its own program
  or read a `var`; `meta_error("...")` stops the compilation with a message. The step budget is 10 million, or `@meta_limit(n)` on the
  declaration. Results are cached per expression; a constant that needs itself is an error. Owner answers (recommendations used): reflection
  later, budget 10 million with `@meta_limit`, baked data is built where it is used (a copy), `target` fields as listed above. A failing compile-time run reports its call chain
  (`... (in inner <- outer)`). Not built: static data with relocations (the code that builds the value is generated instead).

### Built so far: async at stage 0 (card A-07)

`async fn f(...) -> T` is called like any function and gives a `future<T>`; `await e` (only inside an `async fn`) consumes the future and gives
the `T`; `block_on(future)` does the same from ordinary code. At this stage a call **runs the body to the end** (like the cooperative tasks
of stage 0, IR18) and the future holds the result, so `await` never has to wait. `await f()?` means `(await f())?`. Futures have the layout
of a `T`, and no executor exists. What is not built: suspension in the middle of a body (a state machine per `async fn`, with the rule
that no view or `inout` lives across an `await`), `async` closures and blocks, `select` over futures. Those wait for real threads (S-02)
and the backend; the card is S-04.

## 16. Hints to the optimizer

```
@inline fn square(x: int) -> int = x * x
@noinline fn rarely_used() { ... }
@cold fn report_failure() { ... }
```

Hints are written as attributes (`@name`) in front of a declaration. They are
requests: by default the compiler decides, and it usually knows better. `@inline`
asks the compiler to expand the function at every call.

## 17. Async functions

```
async fn fetch(url: string) -> string ! net_error {
    const reply = await connect(url)?
    ...
}
```

Async is the hardest item on this list, because a function that can pause must keep
its locals somewhere, and under value semantics a borrowed `inout` cannot be held
across a pause. Rust solved this with considerable complexity. My recommendation:
do not build async for version 1. Use threads and channels first (below), reserve
the words `async` and `await`, and revisit after the compiler is self-hosting.

Threads fit value semantics well. A closure sent to another thread must take what it
uses by copy or by `sink`, and mutation needs exclusive access, so a data race cannot be
written down:

```
const results = chan<int>()
spawn(sink results ~> results.send(heavy_work()))
```

(Sketch only; the concurrency page comes later.)

## 18. Same name, different shape (overloading)

We decided no overloading of function names. What covers the usual needs:

- **Optional parameters:** defaults and named arguments (section 5).
- **Different types:** generics and interfaces (section 9).
- **Operators for your own types:** an interface per operator, such as `impl add for vec2`.
- **Several ways to create a value:** *associated functions*, which are functions inside a type
  without `self`, called on the type itself:

```
struct point {
    x: float
    y: float

    fn polar(radius: float, angle: float) -> point { ... }
}

const p = point.polar(1.0, 0.5)
```

---

## 19. Closures that can be called only once (decided and built 2026-10-20)

**The problem (checked on `main`, 2026-10-20).** A closure that only *reads* what it captured with `[sink s]` can be called many times; that already works. A closure whose body *consumes* a capture (`~> eat(sink s)`, or returns `s`) runs the body once on the captured value; a second call reads memory that is already gone, and nothing reports it. `spawn`, tasks and `defer` in `core.pel` rely on such one-shot closures, so they cannot simply be forbidden.

**Proposal (the owner chose a type form, "once fn").**

```
fn run_later(f: once fn() -> int) -> int = f!!()     // a one-shot function value
const g = [sink s] () ~> eat(sink s)                  // inferred: once fn() -> string
const h = [sink s] () ~> s.len()                      // inferred: fn() -> int (reads only)
```

- `once fn(A) -> R` is a function type that can be called once. `fn(A) -> R` is a subtype: a many-call closure can be passed where a `once fn` is expected, never the other way round.
- A closure expression gets `once fn` when its body consumes a `[sink]` capture, and plain `fn` otherwise. Nothing to write at the closure.
- A `once fn` value is owning: it moves, cannot be copied, and *calling it consumes it*. The call is marked like other consumers, `f!!()`, so a second call is an ordinary use-after-move error from the existing value-semantics pass.
- A `once fn` that is never called is dropped like today (the environment frees its captures).
- Library: `spawn`, `scope.spawn`, `defer` and the task body take `once fn`.
- Representation does not change (same environment, same `taken` flag); the new thing is a flag on the function type in the checker, plus one parser rule for the type.

**Work (done):** parser (`once fn` type), `ast_print.rs`, `visit.rs`, `TyKind::Fn` gets a `once` flag with subtyping in `unify`/coercion, inference in the checker, the call rule in `values/`, `core.pel` signatures, `examples/tour.pel`, tests, golden trees.

The checker finds the consuming forms that are written out (`sink x` argument, `!!` call on `x`, `x` returned); any other move of a `[sink]` capture is refused by the value pass until the context gives the closure a `once fn` type.

**Decisions (owner, 2026-10-20: "go with yours").**

1. Spelling is `once fn(int) -> int`; `once` stays a soft keyword, special only before `fn`.
2. Calling is `f!!()`, so consumption is visible at the call site.
3. Inference only; `once` is not written on closure expressions.
4. `defer` and `meta` closures stay as they are in version 1; `spawn`, `scope.spawn` and the task body take `once fn`.

---

## Decisions from draft 2

- **G1** Variadic parameters are written `...nums: int`, spread with `sum(...xs)`.
- **G2** Tail calls are explicit and guaranteed: `become f(x)`.
- **G3** `meta` has four uses: `meta expr`, `meta fn`, `meta N: int` as a generic value parameter, and `meta if`. The `when` keyword is dropped.
- **G4** Optimizer hints are attributes (`@inline`, `@noinline`, `@cold`), not keywords.
- **G5** Async was deferred; the owner asked for everything to be completed, so stage 0 now has `async fn` and `await` (see the note after section 17): a call runs the body to the end and gives a `future<T>`, `await` takes the value out. True suspension (a state machine per `async fn`) is still future work (card S-04).
- **G6** Several ways to create a value are associated functions (`point.polar(...)`).
- **G7** Consuming a value looks different from changing it.

## Decided since

- **Consuming a value is marked `!!`:** `c.close!!()`. Changing stays `c.bump!()`.
- `sink` arguments are repeated at the call site like `inout` ones: `finish(sink scores)`.
