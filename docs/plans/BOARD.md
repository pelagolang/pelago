# Board

Last updated with the plans written after the lowering milestone (IR9 to IR20 in `docs/design/ir.md`).

**Order of work.** Track A goes strictly in this order: `A-01`, `A-02` (box), `A-03` (temporaries), `A-04` (IR optimiser),
`A-05` (meta), `A-06` (soa), then `A-07` (async) only after the compiler is self-hosting. `A-08` is a small checker fix and should be taken right after `A-02`. Track B is independent of A and runs
in its own order: `B-01` (name), `B-02` (extension), `B-03` (highlighter), `B-04` (rename the crate and docs).

## Track A: compiler

| Column | Card | Title | Size |
|---|---|---|---|
| **Needs decision** | [A-02](cards/A-02-box.md) | `box<T>`: make, read, move out (owner answers 6 questions) | L |
| **Ready** | [A-01](cards/A-01-cleanup.md) | Remove `stubs.rs` and other leftovers | S |
| **Ready** | [A-03](cards/A-03-temporaries-ledger.md) | The ledger: no leaked temporaries (our own ownership model) | L |
| **Ready** | [A-04](cards/A-04-ir-optimiser.md) | IR optimiser: slots to registers, drop flags away, inlining | XL |
| **Needs decision** | [A-05](cards/A-05-meta.md) | `meta`: compile-time code (stages M1 and M2 can start now; 4 questions gate M3) | XL |
| **Ready** | [A-06](cards/A-06-soa.md) | `soa<T>`: struct-of-arrays collection (after A-05 in order; 3 small questions) | L |
| **Inbox** | [A-08](cards/A-08-checker-closures.md) | Checker bugs: `inout` closure parameters, the `scope` handle type (found while testing locks) | S |
| **Shelf** | [A-07](cards/A-07-async.md) | `async` / `await` (deferred by decision G5) | XL |

## Track B: tooling and identity

| Column | Card | Title | Size |
|---|---|---|---|
| **Needs decision** | [B-01](cards/B-01-name.md) | Choose the language name (research done; shortlist inside) | S |
| **Inbox** | [B-02](cards/B-02-extension.md) | Choose the file extension (after B-01) | S |
| **Ready** | [B-03](cards/B-03-highlighter.md) | Syntax highlighter: TextMate grammar, VS Code extension, `langc highlight` | M |
| **Inbox** | B-04 | Rename crate, binary, docs and examples to the chosen name (mechanical; after B-01 and B-02) | S |

`B-03` can start before the name is final: it uses the placeholder extension `.lang`, and renaming it later is one search and replace.

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
| [D-01](cards/D-01-lowering.md) | Lowering of the whole checked language to IR, interpreter, `langc run`, leak checks | `compiler/src/lower/`, `docs/design/ir.md` IR9 to IR20 |
| D-02 | Checker stages 1 to 6, value semantics, exhaustiveness | `docs/design/checker.md` |
| D-03 | Lexer, parser, printer, visitors, golden tests | `docs/design/lexer.md`, `parser.md` |

## Doing

Nothing right now.

## Review

Nothing right now.
