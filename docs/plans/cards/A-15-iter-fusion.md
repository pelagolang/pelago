Id: A-15
Title: Lazy iterators by fusion in `for` loops
Column: Doing
Order: 15
Depends on: A-13 (the `chars()` loop, PR 163)
Size: M
Plan: [docs/design/iter.md](../../design/iter.md)

# What

`for x of <source>(.map(f) / .filter(p))*` becomes one loop that pulls from the source in place, so no temporary lists are built.
Stages IT1 to IT5 are in the plan; IT1 (`chars()`) is merged.

# Done means

- IT2 to IT3 merged with tests in the interpreter, optimiser and native; `strsplit` measured once.
- Elements are processed one at a time; `break` stops the source; overflow traps unchanged.
- `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` green.

# Progress

- IT1 done in PR 163.
- Plan written (this card and `docs/design/iter.md`).
- IT2 done: `for w of s.split(sep)` and `for b of s.bytes()` fused (`lower/seq.rs` `for_fused`, library `rtSplitEnd` and `rtSplitAfter`; the eager `split` uses the same helpers).
- IT3 done: `for x of src.map(f).filter(p)...` runs element by element in `lower/seq.rs` (`peel_adapters`, `eval_adapters`, `apply_adapters`). Fused only when the closure is a literal or a name and neither the input nor the output elements need drop glue; the function values are made once before the loop; `break` stops the source (a `map` closure does not run on elements after it). Any other chain stays eager. Test: `for_over_map_and_filter_runs_element_by_element` (interpreter, optimiser, native). No benchmark row uses a chain, so none was measured.
