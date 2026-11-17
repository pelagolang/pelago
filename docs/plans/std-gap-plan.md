# Standard library gap plan (modelled on Rust's `std`)

Status: draft for the owner. Card: [S-15](cards/S-15-std-library.md). Requested by Dennis (2026-11-17): "a proper std lib like Rust", grouped by area, not a flat list.

Out of scope here (other threads own them): the time library (`std.time`), crypto and hashing (`std.sha2`, `blake*`, `mac`, `kdf`, ...), the website sidebar.

## 1. What exists today

Two packages. **`core`** is always in scope (no import): `list`, `map`, `set`, `str`/`string`, `iter` and its adapters, `option`/`result` methods, number methods (`abs`, `pow`, `sqrt`, ...), `char` tests, wide numbers, tasks/channels/atomics/locks, `arena`, `pool`, `braid`, `box`. **`std`** is loaded when a program imports `std.<name>`; today it is flat: `big`, `io`, `fs`, `os`, `env`, `time`, `random`, `path`, `json`, `regex`, `regexunicode` and the hash family. Modules are one file each, listed in `STD_MODULES` in `compiler/src/package.rs`.

## 2. Rust `std` module by module

"Conv." = we follow Rust closely (conventional). "Own" = Pelago's semantics change the design (value semantics, `let`/`inout`/`sink`, no null, exact overflow traps, no libc, no unwinding, no GC).

| Rust module | Pelago today | Gap | Design note |
|---|---|---|---|
| `option`, `result` | `core` methods | `xor`, `take`, `replace`, `flatten`, `transpose`, `inspect` | Conv. names; no `unwrap` that panics silently: keep `expect`-style stop with a message |
| `iter` | `core` adapters (A-15 fusion) | `step`, `windows`, `chunks` on iter, `scan`, `partition`, `unzip`, `product`, `maxBy`, `minBy`, `sumBy`, `dedup`, `peekable`, `cycle`, `repeat`, `once`, `successors`, `fromFn` | Conv. API; fused in `for` loops (Own: no boxed iterator objects) |
| `vec`/`slice` | `list` | `retain`, `drain`, `splitAt`, `rotate`, `fill`, `first/last`, `concat`, `partition`, `isSorted`, `sortUnstable`, `windows`, `copyWithin`, `startsWith` | Conv. |
| `string`/`str`/`char` | `core` | `splitN`, `rsplit`, `matches`, `charIndices`, `trimMatches`, `isAscii`, `eqIgnoreCase`, `toTitle`, `char` classification beyond ASCII (Unicode tables), `escape`, `fromUtf8`/lossy | Conv. names; Own: UTF-8 always, byte index API checks boundaries |
| `fmt` | `show` interface, `#()` strings | padding, width, precision, hex/binary/octal, `{:?}`-style debug (`debug` interface), `fmt.table` | Own: `#(x:08.3)` format spec inside the interpolation instead of a `format!` macro |
| `cmp`, `ops` | `eq`, `order` interfaces | `min`, `max`, `clamp`, `reverse`, `compareBy`, `ordering` enum (`less/equal/greater`) | Own: `ordering` enum instead of bare ints, keep int-returning `cmp` working |
| `default`, `clone`, `convert` | `copy`, partial | `default` interface, `from`/`into` conversions with checked narrowing | Own: narrowing is `toByteChecked`-style; no implicit conversions |
| `num`, `f32/f64`, `i32`.. | `core.numbers` | `checkedAdd/SubMul`, `wrappingX`, `saturatingX`, `overflowingX`, `leadingZeros`, `trailingZeros`, `popcount`, `rotateLeft/Right`, `swapBytes`, `toBeBytes/fromBeBytes` (+Le), `isqrt`, `gcd`, `lcm`, `signum`, `rem_euclid`, float `isNan`, `isFinite`, `sin`..`atan2`, `exp`, `ln`, `log10`, `hypot`, `toBits/fromBits`, `parse` for every width and radix | Own: arithmetic traps on overflow by default, the `x` variants are the explicit opt-outs; no libc, so the float functions are written in Pelago |
| `collections` | `list`, `map`, `set` (insertion-ordered) | **`deque`**, **`btreeMap`/`btreeSet`** (sorted), **`binaryHeap`** (priority queue), `linkedList` (probably skip), `bitSet`, `smallList` | Conv. API. Own: `map`/`set` stay insertion-ordered (deterministic iteration, no randomised hasher); entries via `inout` API instead of Rust's borrowing `entry` |
| `hash` | `hash` interface, hasher family (crypto thread) | `std.hash.hasher` for map keys is core; nothing here | Crypto thread |
| `io` | `std.io` (stdin, stderr, `file`) | `reader`/`writer` interfaces, `bufReader`, `bufWriter`, `lines`, `copy`, `cursor` (in-memory), `stdout` handle, `seek`, `readExact` | Conv. shape. Own: `file` closes on scope exit already; writers are `inout` receivers, no `&mut` |
| `fs` | `std.fs` basics | `copy`, `removeDirAll`, `canonicalize`, `symlink`/`readLink`, `setPermissions`, `tempDir`, `walkDir`, `readDir` returning entries with kinds, `truncate`, `sync` | Conv. Windows file ops still pending in the runtime thread |
| `path` | `std.path` | `relativeTo`, `strip prefix`, `splitExtension`, Windows prefixes, `glob` match | Conv. |
| `env` | `std.env` | `args()`, `currentDir`, `setCurrentDir`, `setVar/removeVar`, `homeDir`, `tempDir` | Conv. |
| `process` | `os.exit` only | `process.run`, `spawn`, `child` with piped stdin/stdout/stderr, `wait`, `kill`, exit status, `id`, `abort` | Conv. API. Own: `child` is an owned value whose `deinit` reaps it; needs runtime syscalls (`fork/exec`/`CreateProcess`) with no libc |
| `thread`, `sync` | `core.tasks` | `std.sync`: `mutex<T>`, `rwLock<T>`, `once`, `barrier`, `condvar`, `oneShot`; `std.thread`: `spawn`, `scope`, `sleep`, `yield`, `availableParallelism` | Own: tasks are stackful and channels are the default; locks own their data and hand out `inout` access in a closure (no guards that can leak) |
| `time` | `std.time` | **separate thread** | |
| `net` | none | `tcpListener`, `tcpStream`, `udpSocket`, `ipAddr`, `socketAddr`, DNS | Last: needs runtime socket syscalls on three OSes; IP/addr parsing and formatting can land first as pure code |
| `ffi`, `os` | `os.exit` | `os.args`, OS error codes, `std.os.linux/windows` raw syscalls (decide with runtime thread) | |
| `error` | `error`, `context` | `error` interface with `source()` chain, `errorList` | Own: `result` + `?` already; add chain display only |
| `any`, `mem`, `ptr`, `alloc`, `pin` | runtime intrinsics (`rt*`) | `mem.swap`, `mem.replace`, `mem.take`, `sizeOf`, `alignOf`; skip `pin`, `any`, raw `ptr`/`alloc` public API | Own: value semantics make `swap`/`replace` trivial; no aliasing so no `Pin` |
| `cell`, `rc`, `sync::Arc`, `borrow` | none | `rc<T>`/`arc<T>` shared ownership only if a real need shows up | Own: parked. Value semantics and arenas/pools remove most uses; propose when a program needs shared immutable graphs |
| `ascii`, `char`, `primitive` | partial | Unicode `char` tables (category, case, width), `ascii` helpers | Tables generated, as for regex Unicode |
| `panic`, `backtrace`, `intrinsics`, `simd`, `arch`, `hint` | stop with `file:line` | `hint.blackBox`, `simd` types (later, with the backend), no `catch_unwind` | Own: no unwinding by decision E3 |
| `json`, `regex`, `big` | done | not in Rust std; Pelago adds them | Pelago extras |
| `random` | done (`std.random`) | not in Rust std | Pelago extra |

## 3. Layout: one `std`, grouped

Dennis does not want a flat list. Proposal: dotted groups, one import path per module, like Rust's `std::collections::btree_map`:

```
std.collections.{deque, btree, heap, bitset}
std.io.{reader, writer, buffered, cursor}
std.fs  std.path  std.env  std.process
std.sync.{mutex, rwlock, once, barrier, condvar}   std.thread
std.num.{checked, bits, float, parse}     std.fmt     std.text.{unicode, ascii}
std.net.{ip, tcp, udp}                    std.hash.* and crypto (crypto thread)
std.json  std.regex  std.big  std.random  std.time
```

Today a module is a flat name in `STD_MODULES` (`import std.json`). **Step 0** is loader support for nested names (a directory per group in `compiler/std/`, `std/collections/deque.pel` is `import std.collections.deque`), keeping the current flat imports working as aliases so nothing breaks. The website thread then builds the foldable "std" tree from the same list (one source of truth: `STD_MODULES` gets a `group` field and `pelago std --json` prints the tree).

## 4. Batches (one PR each, tests as `.pel` + `.out` in `std/tests`)

0. **Layout**: nested module names, group metadata, `pelago std` listing. Aliases for existing flat names.
1. **Numbers** (`core`): checked/wrapping/saturating/overflowing, bit ops, byte conversions, `gcd`, float maths without libc, `parse` for all widths/radixes.
2. **Collections**: `deque`, `btreeMap`/`btreeSet`, `binaryHeap` (the long-tail items already listed in memory).
3. **Iterators, lists, strings, option/result leftovers** (table above), `ordering`, `default`, `mem`.
4. **`fmt`**: format specs in `#()`, `debug` interface.
5. **`io` traits and buffering**, `fs` leftovers, `path`, `env` leftovers.
6. **`sync` and `thread`** on top of the existing tasks and atomics.
7. **`process`** (needs runtime syscalls on Linux, Windows, macOS).
8. **`net`** (IP types first, sockets last).
9. Unicode char tables, `rc`/`arc` only if a program asks.

Each merged module: ask the coordinator to get it documented on the website and verify on the live site. Examples stay compiling (`examples/programs/` must type check).

## 5. Where this is conventional and where it is Pelago's own

Conventional (follow Rust): module split and names, the `collections` family, `Option`/`Result`/`Iterator` method names, `io` reader/writer split, `process.Command` shape. Own: overflow traps with explicit `wrapping`/`checked` opt-outs, insertion-ordered hash maps, `ordering` as an enum, format specs inside string interpolation, `inout`-closure locks instead of guards, owned `child`/`file` values closed by `deinit`, no `Rc`/`RefCell`/`Pin` family, no unwinding.

## 6. Questions for Dennis

1. Nested module paths (`std.collections.deque`) with flat aliases kept: yes? (recommended)
2. `ordering` enum next to the int-returning `cmp`: add it, keep `cmp` as is? (recommended)
3. `map`/`set` stay insertion-ordered; `btree` gives sorted order: agreed?
4. `rc`/`arc`: park until a real program needs them? (recommended)
5. `net` last, after `process`: OK?
