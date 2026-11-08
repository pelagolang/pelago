Id: S-08
Title: Typed JSON decoding and file input
Column: Doing
Order: -
Depends on: `std.json` (done), `std.io` (done)
Size: M, in four steps
Revisit when: `meta` reflection exists (the derive can move into the library).

# Why

`json.parse` gives a tree of `json.value`; programs still pick it apart by hand, and numbers above 2^53 are rounded. The owner asked for typed decoding into declared structs and enums, and for file and standard input (2026-11-08).

# Plan

Design and decisions: [docs/design/json.md](../../design/json.md). Steps: (1) design, (2) `io.readFile`, (3) exact integers, `json.reader`, positions, (4) `@derive(fromJson)`.
