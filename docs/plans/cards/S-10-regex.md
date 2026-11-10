Id: S-10
Title: Pelago's own regex engine
Column: Doing
Order: -
Depends on: `std.regex` (the Pike VM, done), `derive.rs` (the source-text hook, done)
Size: L, in five steps
Revisit when: `meta` can declare types (the generator can move out of Rust into the library).

# Why

The owner wants his own engine, not only a Pike VM (2026-11-08, go given 2026-11-10): a regex literal compiled to Pelago code, named captures as typed struct fields, memory known at compile time, time linear in the text. The Pike VM stays as the reference.

# Plan

Design and open questions: [docs/design/regex.md](../../design/regex.md). Steps: (1) reference extended with named groups and lookahead (done), (2) reference completed (flags) and a corpus checked against Python, (3) pattern front end in Rust, (4) generator and `@regex`, (5) the two engines agree on the corpus, then switch for literals. Speed tuning and the DFA are Optimise work; Owner's answers (2026-11-10): strict integer captures, `@regex` on a struct, Unicode as a follow-up card, backreferences wanted with the tiers and budget of design section 5.
