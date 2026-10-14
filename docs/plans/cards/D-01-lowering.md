Id: D-01
Title: Lowering of the whole checked language
Column: Done
Order: -
Depends on: -
Size: XL
Touches: lower, ir, typecheck, core.lang

Built: `compiler/src/lower/` (about 20 files), the IR interpreter, `langc run`, `compiler/core/core.lang` (list, text, map, set,
range, option, result, task, channel, shared, lock), about 65 lowering tests with leak checks. Design record: `docs/design/ir.md`, IR9 to IR20.

Known limits carried forward: tasks run in sequence (S-02), map removal is O(n) (S-01), values that are never bound can leak (A-03),
slots and drop flags are everywhere (A-04), `box` values (A-02), `meta` (A-05), `soa` (A-06), `async` (A-07).
