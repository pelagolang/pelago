Id: A-01
Title: Remove `stubs.rs` and other leftovers
Column: Done
Order: 1
Depends on: nothing
Size: S
Touches: lowering only

# Why

After the lowering milestone, `compiler/src/lower/stubs.rs` still holds one function, `obj_literal`. Nothing calls it
in practice: objects are built by `obj_literal_value` in `obj.rs`, reached through the scalar path. The file's header
says "Placeholders for the parts of the lowering still to be written", which is no longer true and misleads readers.

# Steps

1. In `agg.rs`, `struct_lit` calls `obj_literal` when the type is an `obj`. A struct literal reaches `struct_lit` only for memory types
   and an `obj` is a scalar, so that branch is dead. Replace it with an internal error message ("an object literal reached the memory path") so a future bug
   is loud, not silent.
2. Delete `stubs.rs` and its `mod stubs;` line in `lower/mod.rs`.
3. Search `compiler/src/lower/` for `unsupported(` calls whose text says "not lowered yet" for something that now works, and for comments
   that say "stub" or "placeholder". Fix or delete them.
4. The code comment in `glue.rs` near `clone_body` and the leftover word "Library types" (removed) must not come back; read the file once for stale comments.
5. Update `docs/design/ir.md` section 8 only if a sentence there still mentions stubs.

# Done means

- No file called `stubs.rs`; `grep -rn "stubs" compiler/src` finds nothing.
- fmt, clippy and tests are green; the number of tests is unchanged.
