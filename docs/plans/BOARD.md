# Board

Last updated when every card on the first list was finished (A-01 to A-08 and B-03). What is left is on the shelf: it waits for the backend or for a measured need.

**Order of work.** The next real step is the machine-code backend: card [A-09](cards/A-09-native-backend.md) is under way (owner's answers recorded in the card): x86-64 machine code for Linux and Windows. Done: N1 to N5 and the Windows target; the leftovers (buffered print, float semantics, stack guard) are item W8 in [a11-work-items.md](a11-work-items.md). The editor work (S-05, S-06) is built and in Review.

## Track A: compiler

| Column | Card | Title | Size |
|---|---|---|---|
| Doing | [A-10](cards/A-10-aarch64.md) | AArch64 (64-bit ARM) backend: R1 (encoder) and R2 (Linux ELF, runtime, generator, tests under qemu and on ARM CI) built; R3 (registers) built; Windows on ARM built; macOS next | L |
| Doing | [A-11](cards/A-11-native-speed.md) | Native speed: S1 to S3 first rounds done (collatz 0.30 s to 0.09 s, C -O2 0.07 s); open work split in [a11-work-items.md](a11-work-items.md) | XL |
| Done | [A-09](cards/A-09-native-backend.md) | Native backend: x86-64 machine code, ELF and PE, straight from the IR (all stages and the leftovers: buffered print, float semantics, stack guard) | XL |

## Track B: tooling and identity

| Column | Card | Title | Size |
|---|---|---|---|

## Shelf (parked on purpose)

| Card | Title | Revisit when |
|---|---|---|
| [S-01](cards/S-01-map-storage.md) | Our own map and set storage: O(1) removal, still insertion-ordered | A benchmark shows removal or iteration is slow, or the backend exists |
| [S-02](cards/S-02-real-threads.md) | Real threads for tasks | The machine-code backend can emit atomics and thread start |
| [S-03](cards/S-03-limits.md) | Smaller limits: many-call closures, static tables, a few more | Each line names its own trigger |
| [S-04](cards/S-04-async-suspension.md) | async: suspension in the middle of a body (state machines) | Real threads exist, or a program needs many concurrent waits |

## Done

| Card | What | Where |
|---|---|---|
| [S-07](cards/S-07-formatter.md) | `pelago fmt` and LSP formatting: token layout, `[fmt]` in `pelago.toml`, wrapping, import sorting, tree-based indentation | `compiler/src/fmt/` |
| [S-05](cards/S-05-zed-and-tree-sitter.md) | Tree-sitter grammar (`pelagolang/tree-sitter-pelago`) and Zed extension (`pelagolang/zed-pelago`), the VS Code extension split out to `pelagolang/vscode-pelago`; done (owner) | the three repositories |
| [A-01](cards/A-01-cleanup.md) | `stubs.rs` removed | `compiler/src/lower/` |
| [A-07](cards/A-07-async.md) | `async fn` / `await` / `future<T>` / `block_on` at stage 0 (eager); suspension is S-04 | `core.pel`, IR26 |
| [B-03](cards/B-03-highlighter.md) | Highlighter: `pelago highlight`, generated TextMate grammar, VS Code extension | `compiler/src/highlight.rs`, `editors/` |
| [A-05](cards/A-05-meta.md) | `meta`: `meta if` and `target`, `meta N`, `meta expr`, `meta fn`, `meta_note`/`meta_error`, `@meta_limit`; example `tables.pel` (CRC table at compile time) | `lower/meta.rs`, `meta.rs`, IR25 |
| [A-06](cards/A-06-soa.md) | `soa<T>`: columns per field, element views, loops, library operations (IR24); blocked layout left for the backend | `lower/soa.rs` |
| [A-04](cards/A-04-ir-optimiser.md) | IR optimiser: CFG, DCE, mem2reg, SROA, SCCP, inlining, `--opt`; steps down 30 to 40 percent on the samples (IR23). Remaining: default on, check removal, copies between slots | `compiler/src/ir/opt/` |
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

- [S-06](cards/S-06-language-server.md): `pelago lsp` built (L1 to L5); the owner reviews.
