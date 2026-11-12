Id: S-14
Title: Wide numbers: int128, uint128, int256, uint256, then float16, float128, float256
Column: Doing
Order: -
Depends on: -
Size: L
Revisit when: the optimisation list in docs/design/wide-integers.md is taken up

# Why

Dennis (2026-11-12): "add int128, uint128, float128, etc.", then "add 256. we'll find a way to optimize it", then "float256: I want it".

# Plan

1. **Wide integers** (done in the first PR): one lowering for any number of 64-bit words, no new IR instruction, so the interpreter, x86-64 and AArch64 all work. Design and the optimisation list: [wide-integers.md](../../design/wide-integers.md).
2. **float128** (IEEE binary128) in software, written in Pelago in `core`, on top of the wide integers.
3. **float256** (IEEE binary256, 19-bit exponent, 237-bit significand) from the same code as float128 (width-generic on the wide-integer words).
4. **float16** (half precision), if it fits.
5. Grammars and website for every step; each step its own PR.
