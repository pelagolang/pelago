# Narrow links, byte types and small executables (draft 1)

Goal (owner, 2026-10-27): executables and data as small as `tcc`'s. This page is the size audit
(measured on x86-64 Linux with `pelago build`) and a proposal for narrow links. Nothing here is decided;
the numbered questions at the end are for the owner.

## 1. Audit: where the bytes go

**Executable size.** All 41 `bench/programs` build to 2.6 KB (`loop_sum`) up to 20.5 KB
(`kostya_bf_mandel`); the median is 5.3 KB. `hello` is 3712 bytes: 0x93a of `.text` (the runtime: start,
print, allocator, stop routine), 0x91 of `.rodata`, 0x10a of `.data`, no section tables beyond the minimum.
There is no dynamic linker data, no libc, no unwinding tables. Three things make the larger ones large:

| program | size | why |
|---|---|---|
| `kostya_bf_mandel` | 20456 | 11.7 KB of `.data`: the brainfuck source text is a literal |
| `kostya_json` | 17552 | code: parser, maps, strings |
| `nbody` | 11920 | 10.4 KB of code, mostly the 80-iteration software `sqrt` and checked float code |

So the code-size levers are the `sqrt`/`floor`/`ceil` intrinsics (Goose list item 1) and the usual
machine-code work, not links. `tcc` is not installed in this container (no package source), so the
comparison with it is still missing; the harness has it as a row.

**Memory floor.** Peak resident memory of `hello` was 2072 KiB against 1300 KiB for C. The start code maps
a 1 GiB lazy stack and advises huge pages for all of it, so the first touch of the stack top faulted in a
whole 2 MiB page. Not the heap (first 1 MiB chunk is lazy), not the 64 KiB print buffer (untouched pages
cost nothing). Fixed in PR #138: `hello` is now 1116 KiB, below C.

**Data size.** Byte types already pack: `[uint8]` stores one byte per element (a 5,000,001 element sieve
list is 6.1 MiB peak with `uint8`, 41 MB with `int`). What does not shrink is anything made of pointers:

| thing | layout today |
|---|---|
| `box<T>`, `obj` reference | 8-byte pointer |
| heap block | 16-byte header, sizes 32, 48, 64, ... |
| `enum tree { leaf, node(box<tree>, box<tree>) }` | tag + two pointers = 24 bytes, plus the 16-byte header: a 48-byte block per node |
| `bintrees` (depth 16, 131071 nodes) | 8.2 MiB peak, about 55 bytes per node |

Narrow links attack the last three rows.

## 2. Proposal: pool-relative narrow links

A **link** is an index into a **pool**, stored in 1, 2 or 4 bytes. The pool owns the elements and the
index is plain data.

```
struct node {
    left: link<node, uint16>        // 2 bytes
    right: link<node, uint16>
    key: int32
}

var nodes: pool<node, uint16> = pool()    // at most 65536 slots
const root = nodes.add!(node { left: none, right: none, key: 7 })
nodes[root].key                         // reads through the pool, bounds-checked
```

Why a pool-relative index and not a pointer-relative offset (Goose's way):

- An offset from the link's own address breaks as soon as the struct is copied or moved, which value
  semantics does all the time. An index into a named pool survives every copy.
- The link needs no ownership rules: the pool owns, the link is `Copy`. No dangling owner, no `deinit` per
  node, and freeing the pool frees all nodes in one `munmap` (this is the bump/arena allocator, Goose list
  item 3, and it is a prerequisite).
- Access is `base + index * size`, one multiply-add; the narrow load (zero-extending 1, 2 or 4 bytes) already
  exists in the backend because byte lists use it. The bounds check against the pool length is the same
  check lists have today.
- The width is part of the type, so the checker proves nothing overflows: `add!` stops (like list growth
  past a limit) when the pool is full, and a `uint16` pool cannot hold more than 65536 slots.

Expected effect on `bintrees`: the `node` payload is 1 tag byte + two 4-byte links = 12 bytes in a 16-byte
pool slot with no per-node header, against about 48 today: three times less memory, and one `munmap` instead
of millions of block frees.

What needs building, in order (separate PRs):

1. **Pool allocator** (`pool<T, W>`): lowering to a growable block of slots with a free list, the
   library type, checker rules. No links yet; `pool` gives `add!`, `[]`, `remove!`, `len`.
2. **`link<T, W>`**: a narrow integer in the IR, `none` as an all-ones value (so `option<link<..>>` costs
   nothing), resolved only through its pool (`pool[link]`).
3. **Packed enum/struct layout**, if today's layout pads the 12-byte payload (not yet checked).
4. **Tests and a bench twin** (`bintrees`, `graph`, `lru` with links), reported by peak memory, not time.

## 3. Open questions for the owner

1. Pool-relative links as above (recommended), or Goose-style self-relative offsets inside one allocation?
2. Names: `pool` and `link`, or `arena` and `ref`? (`ref` suggests a reference, which this language
   keeps second-class, so `link` is safer.)
3. Should `link` default its width from the pool (`link<node>` takes `uint32` unless the pool says
   otherwise), or always be written?
4. The pool allocator is its own feature (Goose list item 3). Do it first as a separate card, or only as
   much of it as links need?
