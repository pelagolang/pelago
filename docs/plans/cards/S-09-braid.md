Id: S-09
Title: The braid, end to end
Column: Done
Order: -
Depends on: `docs/design/alternating.md` (decided 2026-11-08)
Size: L, in one go (owner's choice)
Revisit when: lazy `iter<T>` exists (strand views for `for inout`), or someone needs `first`/`second` for a cycle longer than two.

# Why

The owner invented a fixed-size sequence whose types alternate, `(int, string; 3)`, named it **braid**, and asked for the whole system as one card: the type, literals, slot rules, operations, layout, JSON and docs (2026-11-08).

# Done

Parser (`braid<A, B; N>`, repeat literal), checker (literals by annotation, slot rules, `braid_rules`, `/` and `%` in type-level numbers), lowering (N fields, glue), library `core.braid` (`braid.zip`, `braid.fromLists`, `first`, `second`, `tail`, `split`/`unbraid`, `strands`, ...), `list.toArray`/`asArray`, `@derive(fromJson)` for braids, tuples and `[T; N]`, `json.braidItems`. What shipped and what differs from the draft: section 10 of the design.
