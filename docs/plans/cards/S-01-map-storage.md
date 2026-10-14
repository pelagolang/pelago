Id: S-01
Title: Our own map and set storage: O(1) removal, still insertion-ordered
Column: Shelf
Order: -
Revisit when: a benchmark shows removal or iteration slow, the backend exists (so measurements mean something), or a program removes from large maps often.
Size: M
Touches: `compiler/core/core.pel` (all `rt_map_*`), lowering for map iteration, glue

# 1. Today's design and its one weakness

A map or set is a header of four words: `entries block` (keys, then values), `len`, `cap`, `index table`. The index is open addressing with linear probing and holds `entry position + 1` (0 = empty).
Entries are stored **in insertion order**, which the language promises (decision C3: maps iterate in insertion order; `==` ignores order).

Everything is O(1) except **removal**, which shifts every later entry down by one (to keep the order) and rebuilds the whole index: O(n) per removal. A loop that removes many entries from a large map is O(n²).

# 2. The two ideas other systems use (explained, since the names are jargon)

- **Tombstone:** when you remove an entry you cannot just empty its slot in a probing table, because later entries may have been placed *past* it and lookups stop at the first empty slot. So you leave a marker saying
  "something was here, keep looking" — the tombstone. Tombstones make lookups slower as they pile up.
- **Compacting:** now and then, when too many tombstones (dead entries) exist, rebuild the table without them (copy the live entries into a fresh table). It makes removal cheap on average but gives you occasional big pauses and iteration that must skip dead entries.

Plain tombstone-and-compact is the standard answer (Python's dict does it). We do not need to copy it.

# 3. Our own system: the **threaded dense map** with **lazy threads**

Goals: insertion order forever, O(1) insert, lookup and removal, no tombstones in the index, **zero extra cost for maps that are never removed from** (the common case), no pauses.

## 3.1 Shape

```
header:  entries | len | cap | index | order flags/thread info
entries: dense array, position = insertion number until the first removal
index:   power-of-two table of u32 positions (+1), with 8-bit "fingerprints" stored beside them
threads: two u32 per entry (prev, next) — allocated only after the first removal
```

## 3.2 Operations

- **Insert:** hash, probe the index for an empty slot (or the key), append the entry at `len`. If threads exist, link it as the new tail (`next[tail] = pos`, `prev[pos] = tail`, `tail = pos`).
- **Lookup:** hash, probe comparing the stored fingerprint first (one byte compare, rarely a key compare), then the key.
- **Remove (first time):** build the threads in one pass. Positions are still in perfect insertion order, so `prev[i] = i-1`, `next[i] = i+1`: O(n) **once per map**, not once per removal. Then continue as below.
- **Remove (afterwards):**
  1. Unlink the entry from the thread (`next[prev] = next`, `prev[next] = prev`; fix head/tail).
  2. Delete its slot from the index by **backward-shift deletion**: move the following entries in the probe chain back to close the gap when their natural slot allows it. This leaves **no tombstone**; probe chains stay as short as if the entry had never existed.
  3. Fill the hole in the dense array by moving the **last entry** into it (swap-remove): copy the entry, patch its index slot (find it by hash, since we know the key), and patch its two neighbours in the thread. O(1).
- **Iteration:** if there are no threads (never removed from), a plain scan of `entries` — fastest possible and cache-friendly. Otherwise follow `next` links from the head: insertion order, with pointer chasing. Because entries only get out of position by swap-remove, the walk usually still moves mostly forward through memory.
- **Growth:** double the entry arrays and rebuild the index; threads copy as they are (positions do not change on growth).
- **Clear:** reset `len`, drop threads, wipe the index.

## 3.3 Why this beats the standard answer for us

| | Tombstone + compact | Our threaded dense map |
|---|---|---|
| Remove | O(1) amortised, with rebuild pauses | O(1) worst case after a one-time O(n) conversion |
| Pauses | Yes, at compaction | No (except the one conversion, and growth) |
| Never-removed map | Same | Pure array scan, **no thread memory, no checks** |
| Index quality | Degrades until compaction | Stays clean (backward shift) |
| Iteration after removals | Skips dead entries | Follows links (each entry live) |
| Extra memory | none until removals | 8 bytes per entry, only once something was removed |

## 3.4 Extras worth doing in the same pass

- **Hash mixing and masking:** `hash % size` is a division (slow). Use a power-of-two size, mix the hash (multiply by a large odd constant, then take the high bits) and mask. The load factor is 7/8 with backward-shift deletion, or 3/4 to be safe.
- **Cached hashes for strings:** a `string` key's hash is recomputed on every lookup; store the 32-bit hash with the entry when the key type is large (strings, structs).
- **Small-map mode:** up to 8 entries, no index at all; linear scan comparing fingerprints. Most maps in real programs are tiny. Switch to the index at 9 entries.
- **Sets** are maps with no value column (already so); they share all of the above.
- **`soa`-style split** of the fingerprint array from the entries so a probe touches only bytes: later, with the backend.

## 3.5 Tests when it is built

1. Randomised model test against a simple reference (a list of pairs): insert/remove/get/contains/len/iterate over 100,000 random operations; the order after every step must equal the reference order.
2. Removal while iterating is an error already (the checker's exclusivity); test that removal after iteration finishes is fine.
3. Many removals: 10^5 inserts then remove all but one, in random and in insertion order; time must be linear (assert on interpreter step count).
4. Equality ignoring order, hash of key types with collisions (custom `hash` that returns a constant) still correct.
5. The leak and double-free checks on every test.
6. Map with owning keys and values (strings, lists, objects).

# Done means (when someone picks it up)

- The model test and the linear-time test pass; the old behaviour (order, equality, printing) is unchanged in the existing tests.
- `docs/design/collections.md` and `ir.md` (IR12 updated) describe the new layout.
