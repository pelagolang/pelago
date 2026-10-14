Id: A-08
Title: Checker: `inout` closure parameters and the `scope` handle type
Column: Inbox
Order: after A-02
Depends on: nothing
Size: S
Touches: parser (closure parameters), checker (closures, calls), tests

# Found while testing the lowering

1. `lock(0).with(inout n ~> { n += 5 })` is rejected: "the argument `f` only reads; remove `inout`", then "`n` only views part of another value; bind it with `inout n` to change it". The parser reads `inout n ~> ...` as a call-site mark on the whole argument instead of the closure parameter's mode.
   Fix: in a closure's parameter list, `inout` and `sink` before a name set that parameter's mode; the expected `fn(inout T) -> R` type gives the closure its parameter modes even when they are not written. Then the closure body may change `n`.
2. Inside `scope s { s.spawn(...) }` the name `s` has no type, so the closure given to `s.spawn` is checked loosely and its body types are not recorded; the lowering has to special-case `s.spawn`.
   Fix: give `s` a library type with `fn spawn<T>(self, f: fn() -> T) -> task<T>` and let the existing spawn capture rules (disjoint `inout`, no views escaping) apply.

# Tests

- `lock(0)` then `with(inout n ~> { n += 5 })` and read back; `each(inout x ~> { x *= 2 })` over a list.
- `scope` with two `inout` captures of different variables (accepted); the same variable captured by two tasks (error "captured by two tasks"); a view captured and stored (error).
- The special case in `lower/closure.rs` (`run_scope_task`) is deleted afterwards and the scope tests still pass.

# Done means

- Both examples work end to end through `pelago run`; `S-03` lines 7 and 8 are removed; tests green.
