Id: B-05
Title: Port the Islet YAML reader to Pelago
Column: Ready
Order: 5
Depends on: Islet YAML reader and writer (pelagolang/islet), `std.io` file input (done)
Size: M
Phase: Harden (a real program that exercises the library; no new language features)
Touches: `examples/programs/` or a new repository, docs

# Why

The long-term aim is to write Islet in Pelago. Porting one self-contained piece first shows what the library lacks, with a real program and a ready test set (the Rust reader). The YAML reader is the right first piece: text in, a tree or a positioned error out, no network and no hashing.

# Plan

1. Port the strict block-style YAML subset reader (`format: 1` first, plain values read as text, no anchors, tags or inline `{}`) from the Rust Islet to Pelago, with the same error messages and positions.
2. Run both readers over the same inputs (the Rust tests plus the `pelago.yaml`, `islet.yaml`, `monorepo.yaml` and `islet.lock` files in the repositories) and compare the trees byte for byte.
3. List every library gap found (strings, maps, errors) as cards; fix none of them in this card.

# Rules

- **Bootstrap stays on the Rust Islet.** The Pelago one does not replace it until it produces byte-identical stores and locks for the same inputs.
- Later candidate, not part of this card: the BLAKE3 hasher in Pelago (also a speed test against the Rust one).
- Card only for now: nothing is implemented.

# Done means

- The Pelago reader and the Rust reader agree on the whole shared test set.
- A gap list for the library is in this card.
- `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` green; the program is in `examples/programs/` and type checks.
