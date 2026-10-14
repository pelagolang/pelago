# Board

Last updated with the plans written after the lowering milestone (IR9 to IR20 in `docs/design/ir.md`).

**Order of work.** Track A goes strictly in this order: `A-01`, `A-02` (box), `A-03` (temporaries), `A-04` (IR optimiser),
`A-05` (meta), `A-06` (soa), then `A-07` (async) only after the compiler is self-hosting. `A-08` is a small checker fix and should be taken right after `A-02`. Track B is independent of A: the name and extension are settled (Pelago, `.pel`), so only `B-03` (highlighter) is left.

## Track A: compiler

| Column | Card | Title | Size |
|---|---|---|---|
| **Ready** | [A-04](cards/A-04-ir-optimiser.md) | IR optimiser: slots to registers, drop flags away, inlining | XL |
| **Needs decision** | [A-05](cards/A-05-meta.md) | `meta`: compile-time code (stages M1 and M2 can start now; 4 questions gate M3) | XL |
| **Ready** | [A-06](cards/A-06-soa.md) | `soa<T>`: struct-of-arrays collection (after A-05 in order; 3 small questions) | L |
| **Shelf** | [A-07](cards/A-07-async.md) | `async` / `await` (deferred by decision G5) | XL |

## Track B: tooling and identity

| Column | Card | Title | Size |
|---|---|---|---|
| **Ready** | [B-03](cards/B-03-highlighter.md) | Syntax highlighter for `.pel`: TextMate grammar, VS Code extension, `pelago highlight` | M |

`B-03` has no blockers.

## Shelf (parked on purpose)

| Card | Title | Revisit when |
|---|---|---|
| [S-01](cards/S-01-map-storage.md) | Our own map and set storage: O(1) removal, still insertion-ordered | A benchmark shows removal or iteration is slow, or the backend exists |
| [S-02](cards/S-02-real-threads.md) | Real threads for tasks | The machine-code backend can emit atomics and thread start |
| [S-03](cards/S-03-limits.md) | Smaller limits: many-call closures, static tables, a few more | Each line names its own trigger |
| [A-07](cards/A-07-async.md) | `async` / `await` | The compiler compiles itself |

## Done

| Card | What | Where |
|---|---|---|
| [A-01](cards/A-01-cleanup.md) | `stubs.rs` removed | `compiler/src/lower/` |
| [A-03](cards/A-03-temporaries-ledger.md) | The ledger for owning scalar temporaries (objects, boxes); IR22. Memory temporaries already ended with their statement; removing the flags is A-04's job | `lower/func.rs`, IR22 |
| [A-08](cards/A-08-checker-closures.md) | `inout n ~> ...` closure parameters; `scope` handle is the library type `scope_handle`; tasks finish at scope end; a detached task now runs | `core.pel`, `values/mod.rs` |
| [A-02](cards/A-02-box.md) | `box<T>`: `box(v)`, `get`, `set!`, `take!!`, auto-deref, infinite-size check, heap environments for stored closures | `docs/design/types.md` 6a, IR21 |
| [B-01](cards/B-01-name.md) | Name chosen: **Pelago** | `NAMES.md` |
| B-02 | Extension chosen: **`.pel`** | `NAMES.md` |
| B-04 | Rename done: crate, binary, library, examples, docs | this commit |
| [D-01](cards/D-01-lowering.md) | Lowering of the whole checked language to IR, interpreter, `pelago run`, leak checks | `compiler/src/lower/`, `docs/design/ir.md` IR9 to IR20 |
| D-02 | Checker stages 1 to 6, value semantics, exhaustiveness | `docs/design/checker.md` |
| D-03 | Lexer, parser, printer, visitors, golden tests | `docs/design/lexer.md`, `parser.md` |

## Doing

Nothing right now.

## Review

Nothing right now.
