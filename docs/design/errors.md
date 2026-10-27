# Errors and stops (draft 2)

One idea at a time. Syntax is a sketch to react to.

**Already decided and used here:** no exceptions, no null, `result<T, E>` with `ok(...)` / `err(...)`, `option<T>` with `none`,
the shorthands `T ! E` and `T?`, `?` to pass a failure up, `??` for a default, `panic`, `assert`, `todo`, `unreachable`,
`defer`, `deinit`, `switch` and `is` patterns, `!!` for consuming calls, `linear` types, `to` for lossless conversion.

## 1. Two kinds of failure

- **Expected failures** are part of a function's type: a file might not exist, text might not be a number. They come back
  as a `result` or an `option`, and the compiler makes you deal with them.
- **Bugs** are things that should never happen: an index out of range, dividing by zero, a broken promise. They **stop the
  program** (a *stop*), with a message and a stack trace. They are not caught and not part of any type.

The test is simple: if the caller could sensibly do something about it, return it; if the only fix is changing the code, stop.

## 2. Writing an error type

An error type is an ordinary enum (or struct) that provides the `error` interface.

```
enum parse_error {
    empty
    bad_digit(at: int)

    fn message(self) -> string {
        switch self {
            case .empty: "nothing to parse"
            case .bad_digit(at): "bad digit at position #at"
        }
    }
}
```

`@derive(error)` writes a plain `message` from the type's name and fields when you don't need to word it yourself.

## 3. Different errors in one function

A function often meets more than one kind of failure. Two tools, and you can mix them:

```
// 1. A catch-all: `error` holds any error (boxed on the heap, only when an error actually happens).
fn load(path: string) -> config ! error {
    const text = read_file(path)?          // io_error becomes an `error`
    const port = parseInt(text)?          // so does parse_error
    config { port: port }
}

// 2. Your own enum, when callers need to tell the cases apart.
enum app_error {
    io(io_error)
    parse(parse_error)
}

impl converts<app_error> for parse_error {
    fn convert(self) -> app_error = .parse(self)
}
```

`?` converts the error to the function's declared error type if a `converts` impl exists (the catch-all `error` has one for
every error type). Nothing is lost, so it is the same lossless conversion that `x to app_error` performs.

## 4. Adding context

```
const port = parseInt(text).context("reading the port")?
// error shown as: reading the port: bad digit at position 3
const data = read_file(path).context(~> "loading #path")?    // the message is built only if there is an error
```

Context lines stack, so the message reads like a path from the top-level caller down to the cause.

## 5. Handling a result

```
switch parseInt(text) {
    case ok(n): print("got #n")
    case err(.empty): print("nothing there")
    case err(e): print("error: #(e.message())")
}

if parseInt(text) is err(e) { log(e) }

const n = parseInt(text).or(0)             // a default; the error is dropped, visibly
const maybe = parseInt(text).ok()          // result -> option, error dropped, visibly
const found = find(xs, 3).orErr(not_found) // option -> result
```

`??` stays for `option` only. A silent default on a `result` would hide errors, so it is spelled out with `.or(...)`.

## 6. Errors inside closures and builders

`?` returns from the **nearest function or closure**, never from the one outside it. To turn a list of fallible steps
into one result, collect them:

```
const nums: [int] ! parse_error = texts.map(~> parseInt(it)).toList()   // stops at the first error
```

An iterator of results collects into a `result` of a list: the first error wins and the rest are not run.

## 7. Stops

```
panic("unreachable state: #state")        // stop with a message
assert(index < len, "index #index out of range")
unreachable()                             // tell the compiler (and readers) this can't happen
todo()                                    // a placeholder that stops if reached
xs[10]                                    // out of range: a stop
a / 0                                     // a stop
```

- A stop ends the process. There is **no unwinding**: `deinit` does not run and nothing is caught. That is fast, simple, and
  fits linear types (a value that must be consumed doesn't need cleanup on a path that never returns).
- The message and a stack trace go to standard error, and the exit code is non-zero.
- `@on_stop` marks one function that runs just before the process ends, for logging or flushing.
- `test` blocks each run in their own process, so a stop in one test doesn't affect the others.
- Floating-point maths never stops. It follows IEEE: results can be `inf` or `nan`.

## 8. Integer overflow

Overflow is a bug unless you ask for it, so by default it **stops**, in every build, not only in debug builds.

```
const big = a * b                 // stops if it doesn't fit
const hash = a *% 31 +% b         // wraps around on purpose (hashes, checksums)
const safe = a.checkedAdd(b)     // option<int>: none on overflow
const capped = a.saturatingAdd(b)
const wrapped = a.wrappingAdd(b)   // the same wrap as `+%`, as a method
```

The compiler removes checks it can prove unnecessary (loop counters, values it knows are small). `*%`, `+%` and `-%`
are the wrapping operators. Checks can be switched off only inside `unsafe`.

## 9. Cleanup and failing cleanup

- `defer` and `deinit` run on every normal exit, including an early `return` or `?`. They do not run on a stop.
- `deinit` cannot fail (it has no return value), so a close that might fail is offered explicitly:
  `file.close!!()` returns a `result`, and a `linear` file type can force you to call it.

## 10. The program's own result

```
fn main() -> void ! error {
    const cfg = load("app.conf")?
    run(cfg)
}
```

If `main` returns an error, its message (with context) goes to standard error and the exit code is 1.

## What is already our own

- Failure is always visible in a type, and bugs stop the program loudly; nothing is thrown through your stack.
- One conversion rule (`converts`) serves `?` and `to`.
- Overflow is checked in every build, with short operators for the cases where wrapping is the point.
- Stops don't unwind, which keeps the compiler and runtime small and fits linear types.

---

## Decisions from draft 1

- **E1** The catch-all `error` plus your own enums joined by a `converts` impl, used by `?` and `to`. (`converts` is a placeholder name.)
- **E2** `.context("...")` stacks into a message chain.
- **E3** A stop aborts the process with no unwinding; `@on_stop` for a last word; tests run in separate processes.
- **E4** Overflow stops in every build. Wrapping has both forms: the operators `+%`, `-%`, `*%` (and `+%=`, `-%=`, `*%=`) and the methods
  `wrappingAdd` and friends, plus `checked_` and `saturating_` methods. Six new tokens in the lexer.
- **E5** `main` may return `void ! error`.
- **E6** `??` is for `option` only; results use `.or(...)` and `.ok()`.
- **E7** `?` returns from the nearest function or closure; iterators of results collect into a `result`.
- **E8** `deinit` cannot fail; fallible cleanup is an explicit `close!!()` returning a `result`.
