# Sample programs (draft 1)

Nothing here is final. These are written in the syntax we *wish* existed, so we
can react to it. Where a choice is arguable it is marked **[Q1]**, **[Q2]** and
so on, and the alternatives are listed at the bottom.

Defaults used throughout:

- `fn` declares functions, `let` is immutable, `var` is mutable.
- Newlines end statements; no semicolons needed. **[Q1]**
- Types follow the name: `x: Int`. **[Q2]**
- Parameters are read-only by default. `inout` means exclusive mutable access for the call. `sink` takes the value away from the caller.
- `if` and `match` are expressions.
- Files use the placeholder extension `.lang`.

---

## 1. Hello world

```
fn main() {
    print("Hello, world")
}
```

## 2. FizzBuzz (loops, match on a tuple)

```
fn main() {
    for i in 1..=100 {
        match (i % 3, i % 5) {
            (0, 0) => print("FizzBuzz")
            (0, _) => print("Fizz")
            (_, 0) => print("Buzz")
            _      => print(i)
        }
    }
}
```

## 3. A struct with methods

```
struct Point {
    x: Float
    y: Float

    fn length(self) -> Float {
        sqrt(self.x * self.x + self.y * self.y)
    }

    fn translate(inout self, dx: Float, dy: Float) {
        self.x += dx
        self.y += dy
    }
}

fn main() {
    var p = Point { x: 3.0, y: 4.0 }
    p.translate(1.0, 1.0)
    print(p.length())
}
```

## 4. An enum with pattern matching

```
enum Shape {
    Circle(radius: Float)
    Rect(w: Float, h: Float)
}

fn area(s: Shape) -> Float {
    match s {
        Circle(r)  => PI * r * r
        Rect(w, h) => w * h
    }
}
```

## 5. A generic function

```
fn max<T: Ordered>(a: T, b: T) -> T {
    if a > b { a } else { b }
}
```

## 6. Closures and functional style

```
fn apply_twice(f: fn(Int) -> Int, x: Int) -> Int {
    f(f(x))
}

fn main() {
    let numbers = [1, 2, 3, 4]
    let doubled = numbers.map(|x| x * 2)     // [2, 4, 6, 8]
    print(apply_twice(|x| x + 3, 10))        // 16
}
```

Closures capture by value (copy or move), never by reference. **[Q4]**

## 7. Interfaces: the OO style, no inheritance

```
interface Shape {
    fn area(self) -> Float
}

struct Circle { radius: Float }
struct Square { side: Float }

impl Shape for Circle {
    fn area(self) -> Float { PI * self.radius * self.radius }
}

impl Shape for Square {
    fn area(self) -> Float { self.side * self.side }
}

// Static dispatch (default): compiled once per type, no runtime cost.
fn print_area<S: Shape>(s: S) {
    print(s.area())
}

// Dynamic dispatch (opt-in): one function for mixed shapes.
fn total_area(shapes: [any Shape]) -> Float {
    var sum = 0.0
    for s in shapes { sum += s.area() }
    sum
}
```

## 8. Value semantics in action

```
fn sum(xs: [Int]) -> Int {              // read-only access, nothing copied
    var total = 0
    for x in xs { total += x }
    total
}

fn push_zero(inout xs: [Int]) {         // exclusive mutable access
    xs.push(0)
}

fn consume(sink xs: [Int]) -> Int {     // caller gives the array away
    xs.len()
}

fn main() {
    var a = [1, 2, 3]
    push_zero(inout a)                  // call site repeats `inout`  **[Q3]**
    let b = copy a                      // b is an independent array  **[Q5]**
    a.push(9)
    print(b)                            // [1, 2, 3, 0]
    print(sum(a))                       // 15
    print(consume(a))                   // a is moved: using it after this is a compile error
}
```

Two things the compiler rejects:

```
push_zero(inout a, inout a)             // error: two exclusive accesses to `a` at once
let c = a                               // allowed only if `a` is never used again (a move)
a.push(1)                               // error: `a` was moved to `c`
```

## 9. Error handling

```
enum ParseError {
    Empty
    BadDigit(at: Int)
}

fn parse_int(s: String) -> Result<Int, ParseError> {
    if s.is_empty() { return Err(ParseError.Empty) }
    var n = 0
    for (i, ch) in s.chars().enumerate() {
        if !ch.is_digit() { return Err(ParseError.BadDigit(at: i)) }
        n = n * 10 + ch.digit_value()
    }
    Ok(n)
}

fn double_input(s: String) -> Result<Int, ParseError> {
    let n = parse_int(s)?                 // `?` returns early on Err
    Ok(n * 2)
}
```

## 10. A slice of the compiler itself (the paper test)

Value semantics forbids storing references in structs, so a syntax tree uses an
arena and integer ids instead of pointers between nodes.

```
struct ExprId { index: Int }

enum Op { Add, Sub, Mul }

enum Expr {
    Num(value: Int)
    Binary(op: Op, lhs: ExprId, rhs: ExprId)
}

struct Ast {
    exprs: [Expr]

    fn add(inout self, e: Expr) -> ExprId {
        self.exprs.push(e)
        ExprId { index: self.exprs.len() - 1 }
    }

    fn eval(self, id: ExprId) -> Int {
        match self.exprs[id.index] {
            Num(v) => v
            Binary(op, l, r) => {
                let a = self.eval(l)
                let b = self.eval(r)
                match op {
                    Add => a + b
                    Sub => a - b
                    Mul => a * b
                }
            }
        }
    }
}

fn main() {
    var ast = Ast { exprs: [] }
    let two   = ast.add(Expr.Num(2))
    let three = ast.add(Expr.Num(3))
    let sum   = ast.add(Expr.Binary(Op.Add, two, three))
    print(ast.eval(sum))                  // 5
}
```

Result of the paper test on this sample: it fits, and arenas are also faster
than pointer-chasing trees, so we lose nothing.

---

## Open syntax questions

**[Q1] Statement endings**
- A. Newlines end statements, no semicolons (used above)
- B. Semicolons required, like Rust
- C. Semicolons optional, like JS (known to cause edge-case bugs)

**[Q2] Type annotations**
- A. `x: Int` (TS and Rust, used above)
- B. `Int x` (C style)
- C. `x Int` (Go style, shortest in tokens)

**[Q3] Marking `inout` at the call site**
- A. Repeat it: `push_zero(inout a)`. Mutation is visible at a glance, which helps readers and AI.
- B. Don't: `push_zero(a)`. Fewer tokens, but mutation is hidden.
- C. Use a sigil: `push_zero(&a)`. Shortest, but looks like Rust references, which these are not.

**[Q4] Closure syntax and capture**
- A. `|x| x * 2` (Rust, used above)
- B. `(x) => x * 2` (TS, but clashes visually with match arms)
- C. `x -> x * 2`
- Capture: by value only (used above). To let a closure change outer state, you pass `inout` explicitly or use a returned value.

**[Q5] Copying big values**
- A. Explicit `copy a` for anything that owns heap memory (used above). Costs are visible. Small types like `Int` and `Point` copy implicitly.
- B. Implicit copies everywhere, with the compiler turning them into moves when it can. Fewer tokens, but silent expensive copies.

**[Q6] Generics**
- A. `<T>` (TS, Rust, used above)
- B. `[T]` (Go, Scala) frees up `<` and `>` as plain comparison operators and makes parsing simpler.

**[Q7] Keywords**
- `fn`, `let`, `var`, `struct`, `enum`, `interface`, `impl`, `match`, `for`, `in`, `if`, `else`, `return` are used above. Any you would rename? (`func`, `function`, `trait`, `protocol`, `switch`?)

**[Q8] Method calls and `self`**
- Methods live inside `struct` blocks (used above) vs in separate `impl` blocks as in Rust.
