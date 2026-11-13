# Soft floats: `float16`, `float128`, `float256`

Decided 2026-11-12 (Dennis: "add int128, uint128, float128, etc.", then "i want it" for `float256`).

## What they are

| Type | Bits | Exponent bits | Significand bits | About |
|---|---|---|---|---|
| `float16` | 16 | 5 | 11 | 3 digits, max 65504 |
| `float128` | 128 | 15 | 113 | 34 digits, max about 1.2e4932 |
| `float256` | 256 | 19 | 237 | 71 digits |

They are IEEE 754 binary formats, implemented in software in `compiler/core/core/softfloat.pel`. No hardware, no libc.
A value is `n` 64-bit words in memory (one word for `float16`, two for `float128`, four for `float256`), like the wide integers.
The lowering (`compiler/src/lower/softf.rs`) calls the core routines; no new IR instruction and no backend change, so the interpreter, x86-64 and AArch64 all work.

## Conventional and Pelago's own

Conventional: the IEEE 754 formats, round to nearest even, subnormals, infinities, NaN, signed zero; unpack, exact integer arithmetic, one rounding on pack; shortest round-trip printing (Steele and White, Burger and Dybvig).
Pelago's own: one routine set generic over (words, exponent bits, significand bits), so all three types share the code; written in Pelago on top of the wide-integer loops.

## Rules

- Literals are rounded once, correctly, at compile time (`compiler/src/softfloat.rs`, a small bignum, no dependencies). `1e4000` is a fine `float128` and a checker error for `float64`.
- `x to T` widens when nothing is lost: any integer type to any soft float (rounding), a float to a wider float. Narrowing is `.toFloat()` for soft to `float`; a narrowing to a soft type of smaller size is not written yet.
- Floats do not trap on overflow: `inf`, `nan` and `-0` behave as in IEEE (the exact-overflow traps are for integers).
- Comparison with `nan` is false except `!=`.
- Printing is the shortest text that reads back to the same value (`0.1` prints `0.1`).

## Checked

The routines were compared with exact rational arithmetic: 750 random cases for binary128 (add, sub, mul, div, rem, bit exact), 400 for binary16 and 12 for binary256, plus literals at all three formats. `compiler/core/tests/soft_floats.pel` keeps a small set.

`sqrt` (correctly rounded: the digit-by-digit integer root of the significand, one rounding), `floor`, `ceil`, `trunc`, `round` (halves away from zero, like `float`), `abs`, `min` and `max` are methods. They were checked bit for bit against exact rational arithmetic (300 random `float16`, 120 `float128`, 12 `float256` patterns, with subnormals, zeros, infinities and NaN). The wide integers have `abs` (signed), `min`, `max` and `pow`.

## Not built

`pow` with a float exponent, `exp`, `log` and the trigonometric functions, a fused multiply add, `switch` patterns and `for` ranges on soft floats (see the switch PR), JSON (see the JSON PR). `%` with a huge exponent gap is slow. Everything is plain loops, correct first; the speed list is in [wide-integers.md](wide-integers.md).
