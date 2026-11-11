Id: S-12
Title: Regex: the leftovers of S-11
Column: Done
Order: -
Depends on: S-11 (done)
Size: S
Revisit when: -

# Why

Three small things from [docs/design/regex.md](../../design/regex.md) section 5 that S-11 did not build (owner asked for them as a follow-up, 2026-11-11).

# Plan (built; the limit is a second positional number, the tier is a warning for tier B and a comment in the generated code)

1. **A limit on referenced groups.** The compiler counts the groups a tier B pattern reads and refuses more than 3 unless the attribute raises it (`@regex("...", groups: 5)`), so nobody gets a high-degree polynomial by accident. Check first that the attribute parser accepts named arguments; if not, a second positional integer.
2. **The tier in `pelago check`.** `pelago check` prints, for each `@regex`, the tier (linear, tier A with the number of alternatives, tier B with the number of referenced groups), so a pattern that fell to tier B is visible.
3. **A budget default.** An optional helper `nameBudget(text)` (`64 * (text.len() + 1)`, the figure in the design) so callers do not invent numbers; the budget stays an explicit parameter.

Closed with the S-13 follow-up: the native-backend panic ("a folded address read as a value") came from a nested scaled index (`(t * 2) * 8`) in `native/regs.rs folds()`; fixed there with a regression test (`an_index_scaled_twice_is_not_folded_into_one_address`).
