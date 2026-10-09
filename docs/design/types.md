# Types: struct, enum, obj, interface (draft 1)

One idea at a time. Syntax is a sketch to react to.

**Already decided and used here:** lowercase built-in types, `struct` is a value and
`obj` is a heap object with inheritance written `:`, `interface` with `impl`, `<T>` generics,
`pub` / `priv` (private by default), `init` / `deinit`, `copy`, `option` / `result`, `self`.

## 1. A struct bundles named values

```
struct point {
    x: float
    y: float
}

const origin = point { x: 0.0, y: 0.0 }
var p = point { x: 3.0, y: 4.0 }
p.x = 5.0                       // fine: p is a var
origin.x = 1.0                  // error: origin is const, all the way down
print("(#(p.x), #(p.y))")
```

A struct is a **value**. Copying it makes an independent one.

## 2. Copying: plain data and owning data

```
var a = point { x: 1.0, y: 2.0 }
var b = a                        // b is its own point; a is untouched
b.x = 9.0                        // a.x is still 1.0
```

A type is **plain** if everything inside it is plain: numbers, `bool`, `char`, other plain
structs, fixed arrays of plain things. Plain types copy implicitly, like `point` above.
A type that **owns** something (a `string`, a list, a map, a `box`, an `obj`) must be copied
with `copy`, so the cost is visible:

```
var names = ["ann", "bo"]
var more = copy names            // a real copy of the list
var moved = names                // allowed only if `names` is never used again (a move)
```

## 3. Methods and associated functions

```
struct point {
    x: float
    y: float

    fn length(self) -> float = sqrt(self.x * self.x + self.y * self.y)
    fn shift(inout self, dx: float, dy: float) { self.x += dx; self.y += dy }

    fn polar(radius: float, angle: float) -> point { ... }     // no self: an associated function
}

var p = point { x: 3.0, y: 4.0 }
p.shift!(1.0, 1.0)               // `!` because it changes p
print(p.length())
const q = point.polar(1.0, 0.5)  // associated function: called on the type
```

## 4. Creating a value with `init`

If a type has rules about what a valid value is, `init` enforces them.

```
struct range {
    lo: int
    hi: int

    init(lo: int, hi: int) -> Self ! bad_range {
        if lo > hi { return err(bad_range) }
        self.lo = lo
        self.hi = hi
    }
}

const r = range(lo: 1, hi: 5)?         // `?` passes the failure up
```

- Calling the type name like a function runs its `init`.
- The `Type { ... }` literal is only allowed where every field is visible. A type with
  private fields and an `init` can therefore only be created through its `init`.
- A failing `init` returns `Self ! error`, using the same `!` shorthand as functions.
- `deinit` is the counterpart: it runs when a value goes away (frees memory, closes a file).

## 5. Fields can have defaults and be private

```
struct config {
    pub port: int = 8080              // public, with a default
    pub verbose: bool = false
    token: string                     // private to the module (the default)
}

const c = config { token: "abc" }     // fields with defaults can be left out
```

## 6. enum: one of several options

```
enum shape {
    circle(radius: float)
    rect(w: float, h: float)
    empty
}

const s = shape.circle(radius: 2.0)
const t: shape = .rect(w: 3.0, h: 4.0)      // the leading dot works when the type is known

fn area(s: shape) -> float {
    switch s {
        case circle(let r): PI * r * r         // arm syntax is still to be designed
        case rect(let w, let h): w * h
        case empty: 0.0
    }
}
```

The compiler checks that every case is handled. `option<T>` and `result<T, E>` are
ordinary enums in the library. An enum can have methods like a struct.

A case can refer to its own type through `box<T>` or an index into a list, like the
syntax tree in the first samples.

## 7. interface: a list of promises

```
interface shape_like {
    fn area(self) -> float
    fn name(self) -> string = "shape"        // a default implementation
}

struct circle : shape_like {
    radius: float
    fn area(self) -> float = PI * self.radius * self.radius
}

impl shape_like for rect { ... }              // the other way to say it
```

An interface can build on another (`interface ordered : equatable`). Generic code
that names an interface (`<T: shape_like>`) is specialised per type with no runtime cost.
`any shape_like` is the opt-in dynamic version (a list of mixed shapes).

## 8. Operators and standard behaviour come from interfaces

```
impl add for vec2 {
    fn add(self, other: vec2) -> vec2 = vec2 { x: self.x + other.x, y: self.y + other.y }
}
// now `a + b` works for vec2
```

Common behaviour can be generated:

```
@derive(eq, order, hash, show)
struct id { value: int }
```

Plain types get `eq` and `copy` automatically. The rest are opt-in.

## 9. obj: a heap object with inheritance

```
open obj animal {
    name: string
    init(name: string) { self.name = name }
    open fn speak(self) -> string = "..."
}

obj dog : animal {
    init(name: string) { super.init(name) }
    override fn speak(self) -> string = "Woof"
}

var a: animal = dog("Rex")
print(a.speak())                       // Woof: the call is looked up at run time
```

- An `obj` lives on the heap and has exactly one owner. Passing it around **moves** it,
  unless a function takes it read-only (the default), `inout`, or `sink`.
- Because the variable holds the object rather than a copy of its fields, a `dog` stored as
  an `animal` keeps its extra data. This is how inheritance fits value semantics.
- A class is closed by default. `open` allows extending it, `open fn` allows overriding.
- `copy` works on an `obj` only if it provides `clone`.
- Testing and converting down: `a is dog` tests. A downcast can fail, so it must return an
  `option`: something like `a to? dog` (spelling open).

## 10. Generic types

```
struct stack<T> {
    items: [T]

    fn push(inout self, x: sink T) { self.items.push!(x) }
    fn pop(inout self) -> T? = self.items.pop!()
}

var s = stack<int> { items: [] }
s.push!(3)
```

## 11. Naming a type

```
type id = int64                 // an alias: the same type with another name
```

For a type that must not be mixed up with its base (metres vs seconds), use a one-field struct.

## 12. Layout (later)

`@packed`, `@align(16)` and `@abi(c)` control memory layout for low-level and C interop work.

## What is already our own

- Plain vs owning types decide copying; the cost is visible in the code.
- `init` that can fail through the type system, and a literal that respects privacy.
- Inheritance that cannot slice, because objects are owned on the heap.
- Operators, equality and printing come from interfaces and `@derive`.

---

## Questions for you

**[T1] The copy rule.** Plain types copy implicitly whatever their size (my recommendation,
with a compiler warning above a size limit), or only up to a size, or only when opted in
(Rust's `Copy`)? A size cutoff has a nasty side effect: adding one field could silently change
what every use of the type does.

**[T2] Enum case spelling.** `shape.circle(radius: 2.0)` always, plus the leading-dot
`.circle(radius: 2.0)` where the type is known. Keep both?

**[T3] Calling `init`.** `range(lo: 1, hi: 5)` (the type name is the call), or `range.new(...)`?
And is `-> Self ! bad_range` the right spelling for a failing `init`?

**[T4] Field privacy.** Private by default, `pub` per field (shown above), or public by default?

**[T5] `@derive`.** Which behaviours come for free on plain data? I suggest `eq` and `copy`
only, with `order`, `hash` and `show` opt-in.

**[T6] Cloning an obj.** `copy o` works only if the obj provides `clone`, otherwise it is a
compile error. Right?

**[T7] Downcasting.** `a to? dog` returning `option<dog>`, or something else?

**[T8] Default field values.** Allowed (shown in section 5)?

**[T9] Interface depth.** Interfaces with associated types and generic methods are powerful
but complex. Leave them out of version 1?
