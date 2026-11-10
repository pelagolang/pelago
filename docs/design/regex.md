# Regular expressions: the reference matcher and Pelago's own engine (draft 1)

Status: step 1 built (2026-11-10), steps 2 to 5 planned. Card: [S-10](../plans/cards/S-10-regex.md).
The owner asked for his own engine, not only a Pike VM (2026-11-08) and gave the go on 2026-11-10.

## 1. What the owner gets

Today (`std.regex`, a Pike VM) a pattern is compiled **when the program runs** and the result is a list of numbers:

```
const re = regex.compile("(?<year>\\d{4})-(?<month>\\d\\d)")?     // runtime, can fail
const hit = re.find(text)?
hit.named(re, text, "year")                                         // string?, looked up by name
```

Next (step 3 and later), a pattern written in the source is compiled **by the compiler** into Pelago code, and the captures are real fields:

```
@regex("(?<year>\\d{4})-(?<month>\\d\\d)(-(?<day>\\d\\d))?")
struct date { year: uint16, month: uint8, day: uint8? }

fn main() {
    const d = dateFind("on 2026-11-10")      // date?  (a pattern that cannot match a given text is a value, not an error)
    print(d.year, d.month, d.day)
}
```

A bad pattern, or a field the pattern cannot fill (`year` is not a group, a group that can skip is not an option, a number that cannot fit `uint8`), is a **compile error at the attribute**. No pattern errors at run time, no string lookups, no allocation per match.

## 2. Decisions and trade-offs

**R1. Two layers, one syntax.** The reference is the Pike VM in `compiler/std/regex.pel`: runtime patterns, simple, the oracle. The own engine reads the same pattern language (one parser, shared) and must agree with the reference on a large generated test set before it replaces it for literal patterns. The reference stays for patterns built at run time (user input, config files).
*Conventional:* the pattern language (Perl/RE2 syntax, so people do not have to learn another), leftmost-first matching, Pike simulation. *Pelago's own:* a literal pattern is compiled by the compiler, captures are typed fields of a struct you declare, a pattern that cannot work is rejected at compile time.

**R2. The surface is `@regex("...")` on a struct you write, not a type made out of thin air.** Same mechanism as `@derive(fromJson)` (`compiler/src/derive.rs`): ordinary Pelago source is appended to the module before it is checked, so it can be read and debugged, and it moves into `meta` when `meta` can declare types. The struct's field names are the group names, its field types say how a group is read: `string`/`str` (text), integer types (decimal digits, range checked, the check is part of the match so `\d{5}` into `uint8` is refused at compile time when it can overflow, or the text simply does not match, see question 1). `T?` for a group that may not take part. Free functions named after the type, as for JSON: `dateFind(text) -> date?`, `dateFindAll`, `dateIsMatch`, `dateReplaceAll(text, f)` (the call-on-a-type form arrives with the checker work from the braid card).
*Alternative considered:* write the type inside the pattern, `(?<year:uint16>...)`. It is shorter but makes the pattern non-portable and hides the type of a field far from the struct. Recommended: the struct.

**R3. The generated code is a tagged NFA over constant tables, with sizes known at compile time.** For a pattern of `m` states and `g` groups the matcher holds two thread lists of `m` entries and `2*(g+1)` capture slots each: fixed arrays on the stack, no `[int]` growth, no per-step copying of capture lists (the reference copies on every `SAVE`). Time is `O(text * m)`, memory is a constant. A byte-class table (equivalence classes of bytes) makes the step a table lookup.
*Later (step 5):* where the number of tagged DFA states stays under a cap (a few hundred), generate the DFA (Laurikari tags): one table lookup per byte, still linear. Over the cap, keep the NFA. This is speed work and belongs to Optimise, but the table format must not make it impossible.

**R4. Lookahead `(?=...)` and `(?!...)` is supported** (built in the reference, step 1). A body is a small anchored match started at the current position. Without care that is `O(text * body)` per lookahead, so the compile-time engine caches the answer per (lookahead, position) in a bit array sized `text.len()`, which keeps the total linear. *Not supported:* lookbehind (`(?<=)`, `(?<!)` are refused with a clear error; fixed-width lookbehind can be added later by matching the body backwards).

**R5. Backreferences are not in this card.** `\1` cannot be matched in linear time, and the owner decides whether they exist at all (question 2). If wanted: an explicit opt-in (`(?r)` or `@regex(..., backrefs: true)`) with a step limit, running on a separate backtracking matcher, never silently mixed with the linear one.

**R6. Bytes, not characters, for now.** Same as the reference: `.` and classes see UTF-8 one byte at a time. Question 3 asks about a `u` mode (code points, `\p{L}`), which fits the class-table design (a UTF-8 automaton) and needs the Unicode tables the library does not have yet.

**R7. Flags.** `(?i)`, `(?s)`, `(?m)` are parsed at the pattern's start and group scope. They are cheap in both engines (a flag changes how a literal or class is compiled). Not built yet; step 2.

## 3. Steps

1. **Reference extended (built, 2026-11-10):** named groups `(?<n>...)` and `(?P<n>...)`, `groupIndex`, `found.named`, lookahead, tests.
2. **Reference completed:** flags `i s m x`, `\A \z`, non-greedy checked against Python on a generated corpus (a script in `compiler/std/tests/` writes the corpus and the expected lines once; the corpus is committed, Python is not needed to run tests).
3. **Pattern front end in Rust:** `compiler/src/regex/` parses and checks a literal pattern (the same grammar as the reference; the two parsers are tested against each other), reports errors at the attribute, binds groups to struct fields.
4. **Generator:** writes the tables and the matching function as Pelago source (like `derive.rs`), `@regex` hooked in `package.rs parse_into`. The IR optimiser and the native backend see ordinary code.
5. **Agree, then switch:** both engines run the corpus; differences are bugs; `regex.compile` of a *literal* is the only place that changes (the std `compile` stays the run-time path). Speed tuning, the DFA and the benchmark (`regex-redux` in `docs/perf/bench-suite-findings.md`) are Optimise work.

## 4. Questions for the owner

1. A number group read into an integer field that does not fit (`\d+` into `uint8`): a compile error when the pattern can overflow it (strict), or the text just does not match (lenient)? Recommended: strict at compile time when the pattern's own digit count can exceed the type, otherwise a failed match.
2. Backreferences: never, or an explicit opt-in on a separate backtracking matcher with a step limit? Recommended: later, only if a program needs one.
3. A Unicode mode (`(?u)`, `\p{L}`, characters instead of bytes): in this card, or a follow-up card after the Unicode tables? Recommended: follow-up.
4. Is `@regex` on a struct the right surface, or do you prefer the type written inside the pattern (R2)? Recommended: the struct.
