Id: S-12
Title: Regex: the leftovers of S-11
Column: Ready (Build)
Order: -
Depends on: S-11 (done)
Size: S
Revisit when: -

# Why

Three small things from [docs/design/regex.md](../../design/regex.md) section 5 that S-11 did not build (owner asked for them as a follow-up, 2026-11-11).

# Plan

1. **A limit on referenced groups.** The compiler counts the groups a tier B pattern reads and refuses more than 3 unless the attribute raises it (`@regex("...", groups: 5)`), so nobody gets a high-degree polynomial by accident. Check first that the attribute parser accepts named arguments; if not, a second positional integer.
2. **The tier in `pelago check`.** `pelago check` prints, for each `@regex`, the tier (linear, tier A with the number of alternatives, tier B with the number of referenced groups), so a pattern that fell to tier B is visible.
3. **A budget default.** An optional helper `nameBudget(text)` (`64 * (text.len() + 1)`, the figure in the design) so callers do not invent numbers; the budget stays an explicit parameter.

Also open, from S-10: the native-backend panic ("a folded address read as a value") on `if (a || b) && state.caps[i * S] == pos` over an array in a struct has no small repro yet; try again when someone is in the backend.
