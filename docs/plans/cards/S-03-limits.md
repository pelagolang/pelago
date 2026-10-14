Id: S-03
Title: Smaller limits and leftovers (the shelf list)
Column: Shelf
Size: various (each line is its own small card when its trigger fires)

How to use this file: when a line's trigger happens, copy the line into a new card in **Ready** and delete it here.
Larger items already have their own cards (S-01 maps, S-02 threads, A-03 temporaries, A-04 optimiser).

| # | Limit | What a fix looks like | Costs speed? | Revisit when |
|---|---|---|---|---|
| 1 | A closure that owns captured data can be called only once and cannot be copied (the body takes over the moved-in values). | Add a second closure kind that keeps its captures for several calls: captured values are *read* by the body, and the environment drops them when the closure drops. Chosen by the checker from what the body does with them (consumes → once; only reads/changes → many). | No. The "many" kind is the normal case and needs no extra work at run time. | A program wants to call an owning closure twice, or `sort_by`/`each` with owned captures. |
| 2 | Object tables, `any` tables and global `var` setup are built the first time they are reached, each use checks a flag. | Emit tables as read-only static data with function addresses as relocations (needs the backend's data sections, and A-05's relocation list in `Module.data`). Global `var` initialisers that are constants are baked; others run before `main`. | Makes it faster: no flag test, no code at the use site. | The backend starts emitting data sections. |
| 3 | `iter` is an eager list: `xs.map(f).filter(g)` builds a list at each step. | Real lazy iterators: a small state struct per adapter, fused by the optimiser (A-04 inlining) into one loop. Builders without brackets already mean "lazy stream" in the design (collections.md section 9). | Makes it faster and uses less memory. | A-04 stage 6 (inlining) is done. |
| 4 | A call chain that recurses deeply (a recursive function, dropping a long chain of boxes) uses the host stack of the interpreter and can overflow it; only self `become` on plain numbers is a loop. | Run the interpreter on a thread with a big stack now; a general tail-call pass in the IR later; iterative drop of self-referential boxes. | None. | A-02 box lands (long chains), or a user reports a stack overflow. |
| 5 | Open-ended ranges (`2..`, `..3`) cannot be kept in a value (a `range` holds two ends and a flag). | Add the missing-end cases to the range layout: a flag byte for "has start" and "has end". Slicing with open ends already works. | A byte of layout. | A program stores or passes an open range. |
| 6 | Spread of a list into call arguments (`f(...xs)` for a non-variadic parameter) is not lowered. | Unpack at the call site into the parameter list (length known only at run time for lists, so only legal for variadic parameters; for arrays it unpacks at compile time). | No. | Someone needs it; confirm the checker rules first. |
| 9 | `to?` works only for `obj` downcasts, not for `any` interfaces. | Compare the table pointer to the table for the target type; return the data pointer wrapped in `option`. | No. | A program wants `x to? dog` on an `any`. |
| 10 | `==`, `<`, `hash` on `any` values are not lowered. | Add `eq`/`hash` slots to the table when the interface requires them. | No. | A program compares `any` values. |
| 11 | Tasks that were never joined run at the end of `main`, after `main`'s locals are gone. | Real threads (S-02) make the question disappear; until then it is documented behaviour. | - | S-02. |
| 12 | Stage-0 `print` of floats prints the shortest form of 6 digits (`0.333333`), not the shortest round-trip form. | Implement shortest round-trip printing (Ryu or Grisu style) in `core.pel`. | Slightly slower printing, better output. | Before the first public release, or when a test needs exact floats. |
| 13 | Every function's parameters of `let` mode that are strings or lists are passed by address and read in place: correct, but a callee that is passed a *temporary* keeps it alive until the call ends. | Covered by A-03 (temporaries die at the end of the statement). | - | A-03. |
| 14 | `@inline`, `@noinline`, `@cold` hints are parsed but ignored. | Carry them into `Func` and use them in A-04's inliner. | None. | A-04 stage 6. |

Notes:

- Items 1, 2, 3 and 14 are *speed* items and each makes the output faster, which is the long-run aim, so none of them is blocked by a "future speed" worry.
- Items 7 and 8 are checker bugs, not limits of lowering. They are listed here so they are not lost; they are small and should be done early.
