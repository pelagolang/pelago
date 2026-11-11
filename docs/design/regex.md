# Regular expressions: the reference matcher and Pelago's own engine (draft 1)

Status: steps 1 to 4b built (2026-11-10), lookahead memoisation and the backreference tiers planned. The owner answered the four questions on 2026-11-10: all as recommended, except backreferences, which he wants (section 5). Card: [S-10](../plans/cards/S-10-regex.md).
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

**R4. Lookahead `(?=...)` and `(?!...)` is supported, in linear time** (built in the reference and in `@regex`, S-11 step 1). A lookahead is a bit per position: "the body matches starting here". Before the main match, each body is compiled *reversed* (concatenations flipped, no capture slots; `^`, `$`, `\b` need no change because an assertion looks at the two bytes around a position, whichever way the scan goes) and run once over the text from the end to the start, with a new thread started at every position. The main matcher reads the bit at its position, so the cost is `O(text * body states)` per lookahead however far the body looks (`(?=.*x)` included). A body that holds another lookahead gets a higher number than it, so inner bits are ready first. Captures inside a lookahead are not kept (as before). *Cost:* the bit table is `lookaheads * (text.len() + 1)` integers, made per call: the one run-time-sized piece of an `@regex` matcher, present only for patterns with a lookahead. *Not supported:* lookbehind (`(?<=)`, `(?<!)` are refused with a clear error; fixed-width lookbehind can be added by the same reversed matching).

**R5. Backreferences are wanted (owner, 2026-11-10); the job is to remove what makes them dangerous.** See section 5 for the research and the plan. In short: they never touch the linear engine; the compiler sorts each pattern into a tier and says which one it got; the expensive tier needs an explicit, deterministic step budget and its result type has a third case, so a program can never take exponential time and can never get a silently wrong answer.

**R6. Bytes, not characters, for now.** Same as the reference: `.` and classes see UTF-8 one byte at a time. Question 3 asks about a `u` mode (code points, `\p{L}`), which fits the class-table design (a UTF-8 automaton) and needs the Unicode tables the library does not have yet.

**R7. Flags.** `(?i)`, `(?s)`, `(?m)` are parsed at the pattern's start and group scope. They are cheap in both engines (a flag changes how a literal or class is compiled). Not built yet; step 2.

## 3. Steps

1. **Reference extended (built, 2026-11-10):** named groups `(?<n>...)` and `(?P<n>...)`, `groupIndex`, `found.named`, lookahead, tests.
2. **Reference completed (built, 2026-11-10):** flags `i s m` (no `x` yet), `\A \z`, and a generated corpus checked against Python (`compiler/std/tests/gen_regex_corpus.py` writes `regex_corpus.pel` and `.out`; the files are committed, Python is not needed to run tests; 500 cases agreed in a manual run, 120 are committed to keep the test fast). Two deliberate differences from Python found by the corpus: `\B` matches in an empty text (Perl and Rust do; Python before 3.14 does not), and `\a` is the letter `a` here, not the bell. The corpus also made `findAll` follow Python 3.7: after an empty match the next match must not be empty at the same place.
3. **Pattern front end in Rust (built, 2026-11-10):** `compiler/src/regexgen.rs` reads a literal pattern (the same grammar and rules as the reference, ported step by step), compiles it to the same tables, binds named groups to the fields of the struct, and reports errors at the attribute: a field without a group, a group that may not take part but whose field is not `T?`, a number field whose group can match more than digits or more digits than the type holds (`\d{3}` into `uint8`: "can match 3 digits, up to 999, but `uint8` holds at most 255"; `\d+` into any integer: "has no largest length"). The test `regexgen_tables_agree_with_the_corpus` compiles every pattern of the Python-checked corpus with the Rust side and runs the tables in the matcher: same answers as Python.
4. **Generator (built, 2026-11-10):** `@regex("...")` appends a self-contained matcher to the module (`package.rs parse_into`, next to `derive.rs`; no import needed): the compiled program as a struct of fixed-size arrays (`[int; M]` for `M` states), the thread lists as `[int; M]` and `[int; M*S]` for `S` capture slots (no growable lists, no per-step allocation, the memory of a match is known at compile time), a Pike simulation written for that size, and the functions `dateFind`, `dateFindAll`, `dateIsMatch` plus a typed builder that reads each group straight from the slots. Example: `compiler/std/tests/regex_attr.pel`. The test `regex_attr_corpus.pel` wraps patterns of the Python-checked corpus in one named group and reads them through `@regex`: same answers as Python in the interpreter and in native code. *Found on the way:* an empty string made by interpolation stopped the interpreter (fixed in #244), and a native-backend panic ("a folded address read as a value") on `if (a || b) && state.caps[i * S] == pos && ...` over an array inside a struct; the generator names the element first as a workaround.
4b. **Still to do in the generator:** the loop is the same algorithm as the reference, so it is linear but not yet specialised to the pattern (no byte-class table, no DFA); lookahead bits come from the reversed scan (R4).
5. **The rest (card S-11):** the reversed-scan lookahead (built, R4); backreference tiers (section 5); then `regex.compile` of a literal can point at the compile-time engine. Speed tuning, the byte-class table, the DFA and the benchmark (`regex-redux`) are Optimise work.

## 4. Questions for the owner (answered 2026-11-10)

1. Number group into an integer field: strict at compile time when the pattern can overflow, otherwise a failed match. **Yes.**
2. Backreferences: **wanted**; research how to remove their cons (section 5).
3. Unicode mode: a follow-up card. **Yes.**
4. `@regex` on a struct you write. **Yes.**

## 5. Backreferences without their cons

**What is wrong with them.** (a) Matching with `\1` is NP-complete in general (Aho 1990), so no engine can be fast on every pattern; a backtracking matcher takes exponential time on `(a*)*\1b`-like input, which is the ReDoS attack. (b) A step limit that is silent or based on a clock gives wrong or irreproducible answers. (c) Captures are no longer fixed-size per match step, which breaks "memory known at compile time". (d) Engines disagree on details: an unset group (`(a)?\1`) fails in Python and matches empty in JavaScript; `\1` before its group is closed. (e) A pattern that uses them quietly makes the whole engine slower, even for patterns that do not need them.

**What can be removed, in order of how much it removes** (our proposal; the first two cost nothing at run time):

1. **Tier A, the group can only match a finite set of strings** (`(a|bc)\1`, `(\d)\1`, `(['"])...\1`): the compiler rewrites the pattern into an alternation of plain patterns (`aa|bcbc`, one branch per string) and compiles it with the linear engine. Linear time, no budget, no opt-in. This covers the most common uses (matching quotes, doubled characters, HTML-like open and close). A cap on the number of strings (say 256) keeps the rewrite small; above it the pattern is Tier B.
2. **Tier B, anything else**: a memoising matcher. A state is (program position, text position, the spans of the *referenced* groups only), and every state is visited at most once, so time is polynomial for a fixed number `k` of referenced groups, of the order `m * n^(2k+1)` for `m` states and text length `n` (the exponent is my reading of Schmid's memory-automata results and is checked when the generator is built; general `k` is where NP-hardness lives). The compiler knows `k`, prints it, and refuses a pattern with `k` above a limit (default 3) unless the attribute raises it. Groups that are not referenced stay out of the key, so a pattern with one backreference is not slowed down by its other captures.
3. **A deterministic budget on Tier B, never a clock.** `@regex("...", budget: 1_000_000)` counts visited states; the default is derived from the text length (`64 * n`). Running out is not an error string and not a wrong answer: the generated function returns a three-case result `matched(T) | nomatch | gaveUp`, so the caller must decide (retry with a bigger budget, reject the input). The same input gives the same result on every machine, so a ReDoS report can be replayed as a test.
4. **One semantics, written down:** an unset group makes `\k` **fail** (not match empty); a reference inside its own group fails; leftmost-first as for the rest of the engine; `\k<name>` and `\1` both work, names preferred in the docs. Differences from Python/Perl are listed in `docs/design/regex.md`, not found by users.
5. **Isolation:** a pattern with a backreference never changes how a pattern without one is compiled, and the tier is in the generated code's comment and in `pelago check`'s output, so nobody gets Tier B by accident.

**What stays a cost.** Tier B is polynomial, not linear: with `k = 2` and 10 000 bytes it can still be 10^12 states, which is why the budget is mandatory and finite. We say so in the docs instead of promising speed.

**Plan.** The reference Pike VM gets a backtracking path with the same memo (step 2b, so the oracle can check Tier A and Tier B against Python on a corpus); the compiler's front end (step 3) classifies tiers; the generator (step 4) writes Tier A as an alternation and Tier B as a memoised function with the budget; the switch (step 5) covers both.
