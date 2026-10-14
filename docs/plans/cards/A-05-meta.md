Id: A-05
Title: `meta`: code that runs while compiling
Column: Inbox (design is decided in `functions.md` section 15 and G3; this card turns it into a build plan; open questions at the end)
Order: 5
Depends on: A-02 (box, for compile-time data structures), A-04 stage 2 or later (a faster evaluator), the lowering (done)
Size: XL (four stages, each shippable)
Touches: parser (already parses), resolver, checker (new "evaluation" of values), a new `compiler/src/meta/`, lowering, docs

# 1. Why it was left out, and why it should not stay out

It was left out of the lowering milestone because it needs an *evaluator inside the compiler*, and until the IR interpreter existed there was nothing to build it on. Now there is one.
`meta` is the language's answer to macros, generics over values, conditional compilation, build-time tables and (later) reflection, so many library designs depend on it:

- `fn zeros<meta N: int>() -> [int; N]` (fixed arrays of a computed size, `soa<T, 8>`, SIMD widths).
- `meta if target.os == linux { ... }` (platform code; replaces the dropped `when` keyword). The standard library cannot be written for several targets without it.
- `const table = meta build_table()` (lookup tables baked into the binary; fast startup).
- `meta fn` helpers that are only legal at compile time.

Not lowering it is a silent promise that none of this works. The syntax already parses and the checker already accepts it loosely.

# 2. What is already decided (do not reopen)

From `docs/design/functions.md` section 15 and decision G3:

- One keyword, four uses: `meta expr`, `meta fn`, `meta N: int` (a value as a generic parameter), `meta if`.
- Compile-time code is **ordinary code of the same language**, run by an evaluator inside the compiler. It is never part of the produced program (still no interpreter in the output).
- No I/O at first, and a **step limit** so a runaway loop is an error, not a hang.

# 3. Approach: reuse the IR interpreter as the evaluator

The IR interpreter already runs any lowered function and checks memory. A `meta` expression is evaluated like this:

1. **Collect** the functions the expression reaches (the lowering already discovers callees through `request_fn`).
2. **Lower** them into a fresh temporary IR module (with `core.lang` as usual).
3. **Run** the entry function in the interpreter with a **step budget** (new interpreter option; default 10 million steps, configurable with an attribute or flag).
4. **Read the result** out of interpreter memory and turn it into a **compile-time value** (section 4).
5. **Bake it in**: scalar results become literals; strings, lists, structs and arrays become **static data** in the output module plus a lowering of "load from that data".

Restrictions enforced by the checker (a "meta context" flag on a function): calls only to other `meta` or plain pure functions; no `unsafe`; no `ext` functions except a whitelist of pure intrinsics (`rt_alloc`, `rt_read`, ...);
no globals that are `var`; no `spawn`/channels; no `print` (a `meta` function that wants to say something uses `meta_note("...")`, a compile-time diagnostic). Allocation inside compile-time code is fine (it is freed there; leaks are an error as in tests).

## 3.1 What counts as a compile-time value

A value is *bakeable* if it is built only from: integers, floats, `bool`, `char`, `string`/`str` (as static bytes), fixed arrays, lists and maps of bakeable values (emitted as read-only static data, then copied on use if the receiver needs
ownership), structs and enums of bakeable values, `option`/`result`. Not bakeable: closures, `box`, `obj`, `any`, tasks, raw pointers. The checker reports "this value cannot be computed at compile time because it holds a closure".

# 4. Stages

## Stage M1: `meta if` and the `target` value (no evaluator needed)

- Add a built-in compile-time record `target`: `os` (`linux`, `windows`, `macos`, ...), `arch`, `pointer_bits`, `debug: bool`, `endian`. Values come from compiler flags, defaulting to the host.
- `meta if cond { A } else { B }`: the condition must be a compile-time expression made of `target` fields, literals, `==`, `!=`, `&&`, `||`, `!`. Evaluate it in the **resolver/checker** (a tiny constant evaluator, about 100 lines).
- **Only the chosen branch is checked and lowered.** The other is parsed (so syntax errors still show) but not name-resolved and not type-checked: this is what lets Linux-only code mention Linux-only functions.
- Allowed at module level (selects declarations) and inside function bodies (selects statements).
- Tests: both branches select correctly; the unselected branch may contain an unknown name without error; a non-constant condition is an error; nested `meta if`; `meta if` inside a generic function.
- Library use: split a platform-specific part of `core.lang` (none today, but the backend will need it).

## Stage M2: value parameters `meta N: int`

- Generic parameters may be values (`<meta N: int>`), usable in types (`[int; N]`, `soa<T, N>`) and in the body as a constant.
- Checker: `ArrayLen` gets a `Param(DefId)` case next to `Known(n)` and `Unknown`; unification of `[T; N]` with `[T; 4]` binds `N = 4`; explicit arguments `zeros<8>()` and inferred ones from array lengths both work.
- The lowering monomorphises on the value: `request_fn` already keys instances on `(def, substitution)`; extend the substitution map so it can hold a value as well as a type (`Subst::Value(i128)`), and make `ArrayLen::Param` resolve through it in `array_len` and layout.
- Arithmetic on parameters in types (`[int; N + 1]`) is allowed only when both sides are compile-time (evaluate with the M1 evaluator extended with `+ - * / %` and comparisons).
- Tests: a function over `[T; N]`, a struct with an array field of size `N`, two instances with different `N` produce two functions, a mismatch is a clear error, `N` used as a loop bound.

## Stage M3: `meta expr` and `meta fn` (the interpreter)

- Parse and check: `const x = meta e` and `meta fn f(...) -> T { ... }`.
- Implement section 3: temporary module, interpreter run with step limit, result extraction, baking.
- Interpreter additions: a **step budget** (counts executed instructions, stops with "compile-time code ran too long"), and a **memory image reader**: given a pointer and a type, walk the layout (the lowering's `Layout` for the type) and produce a `ConstValue` tree.
- `ConstValue` to lowering: scalars become `Const`; aggregates become `Module::add_data` bytes (data blocks already exist and are shared when equal) plus `Op::DataAddr`. Pointers inside data (a string's `ptr`) need **relocations**: write the data blocks with a relocation list that the lowering resolves with `DataAddr`. Use a small `Reloc { offset, data_index }` table in `Module.data`; the interpreter applies relocations when it loads the module, and the backend will emit them as ordinary relocations.
- Dependency order: a `meta` constant may use other `const` values, which are evaluated first (a graph; cycle = error "this constant needs itself").
- Cache: identical `(function, arguments)` evaluations in one compilation run once.
- Tests: a table built by a loop (`[int; 256]`), a string built at compile time, a struct result, a result that holds a closure (error), an infinite loop (step-limit error with the call chain), use of a non-`meta` function that does I/O (error), a leak inside meta code (error), constant referenced from two functions evaluated once.

## Stage M4: library and polish

- Move one real thing to `meta`: for example the CRC or the decimal digit table the library needs, as the showcase.
- Diagnostics: show a compile-time call stack ("in meta call `build_table` at line..., called from ...").
- `meta_note(string)` and `meta_error(string)` for libraries that want to give custom messages.
- Reflection is **not** part of this card (see questions).

# 5. Design risks

- **Two behaviours:** if the interpreter and the future backend disagree on anything (overflow, float formatting), compile-time results differ from run-time results. The differential test from A-04 (interpreter before/after) does not cover this; once the backend exists, add "same result at compile time and run time" tests for every operation.
- **Compile-time cost:** every `meta` run lowers a module. Cache the lowered `core.lang` once per compilation (a shared immutable lowering of the library), not once per `meta` expression.
- **Types that depend on values** make inference harder; keep `meta N` to arrays and `soa` at first.

# Done means (whole card)

- Stage M1 to M4 done; the four uses of `meta` work on the examples in `functions.md` section 15 (`build_table`, `zeros<meta N>`, `meta if target.os == linux`).
- A `meta` infinite loop and a `meta` file read give clear errors.
- `docs/design/functions.md` section 15 updated with what was built; `ir.md` gets IR23 (relocations in data, step limit) ; `ROADMAP.md` line removed from "Still not lowered".
- fmt, clippy, tests green.

# Questions for the owner (answer before stage M3)

1. **Reflection** (`meta` code that inspects a type's fields, to write `@derive` in the library itself): in version 1 or later? **Recommendation: later**; `@derive` stays a compiler feature for now.
2. **Step limit** default and how it is set: 10 million steps, overridable by `@meta_limit(n)` on the declaration? **Recommendation: yes.**
3. **Output of `meta` data:** baked as read-only static data, copied on use. Is that acceptable, or should a baked list be directly usable as a `[T]` view without copying? **Recommendation: copy by default, view when the receiver is read-only (the optimiser can then drop the copy).**
4. **Target flags:** which `target` fields do we promise in version 1? **Recommendation: `os`, `arch`, `pointer_bits`, `endian`, `debug`.**
