Id: S-15
Title: Standard library breadth (modelled on Rust's std)
Column: Doing
Order: -
Depends on: -
Size: XL, in batches
Revisit when: -

# Why

Dennis (2026-11-17): "We need to build a proper std lib like Rust", grouped by area (collections, io, sync, ...), not a flat list.

# Plan

Audit, module table, layout and batches: [std-gap-plan.md](../std-gap-plan.md). Time and crypto are owned by other threads. Each batch is its own PR.
