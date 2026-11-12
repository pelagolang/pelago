# Wide integers: `int128`, `uint128`, `int256`, `uint256`

Decided 2026-11-12 (Dennis: "add int128, uint128, float128, etc.", then "add 256. we'll find a way to optimize it").

## What they are

Four more whole-number types, with exactly the rules of the narrower ones: `int128`, `uint128`, `int256`, `uint256`.

- Every operator of the 64-bit types works: `+ - * / %`, `& | ^ ~`, `<< >>`, the wrapping forms `+% -% *%`, the compares, `++`, `--` and the `op=` forms.
- **Overflow stops the program** (`arithmetic overflow`), a division by zero stops it, and a shift by the width or more stops it. Nothing is silently wrapped except with the `%` operators.
- `x to T` widens when nothing can be lost (the same rule as for `int8` to `int64`: same signedness and not narrower, or unsigned to a wider signed type). `to float` from a wide integer is allowed and rounds to within one step in the last place. Narrowing and `float` to wide are not written yet.
- They print, compare, hash and can be map keys and struct fields.
- Literals: any number that fits the type. A literal larger than `uint128` is only accepted by a 256-bit type.

## How they are built (one path for any width)

A wide integer is a **memory value** of `n` 64-bit words, least significant word first, two's complement, 8-byte aligned (16 and 32 bytes). Nothing in the IR or the backends is new: the lowering (`compiler/src/lower/wide.rs`) is written for any `n`, so a new width is one line in `types.rs` (`IntKind`) and one name in `builtin_type`.

| Operation | How |
|---|---|
| `+ -` | a chain of 64-bit `add`/`sub` with the carry worked out by bit operations (no branches); the overflow test reads the carry out (unsigned) or the sign bits (signed) |
| `& \| ^ ~ !` | one instruction per word |
| `== !=`, `< <= > >=` | the OR of the XORs; the borrow out of the subtraction (a signed compare flips the top bit of both first) |
| `* / % << >>` and printing | `rtWideMul`, `rtWideDiv`, `rtWideShift`, `rtWideShow` in `compiler/core/core/wide.pel`, plain loops over the words; they report overflow with a `bool` so that the stop is reported on the line of the arithmetic |
| conversions | store the words, filling the upper ones with the sign (signed) or zero |
| `==`, `cmp`, `hash` glue | per word |

Five small intrinsics carry the unsigned 64-bit operations the loops need (`rtULt`, `rtUShr`, `rtUMulHi`, `rtUDiv`, `rtURem`); they are single IR instructions, so the interpreter, x86-64 and AArch64 all run the same code.

**Conventional parts:** two's complement words with carry chains, schoolbook multiplication, and the memory layout of "an array of limbs" are what every software big integer does (LLVM legalises `i128` the same way). **Our own parts:** the rule that every operation traps on overflow exactly like `int`, the single width-generic path, and that the library routines hand the overflow flag back to the lowering so the stop is reported at the user's line.

## What is not fast yet (the optimisation list)

Correct first, as agreed. Where the time goes, in the order to attack it:

1. Division is a shift-and-subtract loop (64n iterations); a fast path exists when both operands fit one word. Replace with Knuth algorithm D over 32-bit digits, or a 128 by 64 division using the hardware `div`.
2. Multiplication and shifts are calls; inline the 128-bit case (`mul` and `umulhi` give the product directly) and the shifts by a constant.
3. Operands are copied to private slots before the operation; the optimiser can forward them (a wide integer is not a scalar, so the existing promotion passes do not see it). Promoting wide locals to SSA words is the big step.
4. Printing is double-dabble over the bits (about 64n passes over the digits); use repeated division by 10^19 once the division is quick.
5. A native add-with-carry chain (`adc`/`sbb`, `adds`/`adcs`) instead of the bit-operation carry.

## Limits

- Pattern literals (`switch x { 5 -> ... }`) and `for a..b` ranges over wide integers are not built.
- `float` printing stops at the 64-bit range (a separate, older limit).
- Wide integers are not yet in `@derive(toJson)` and `fromJson`.

## Floats

`float128` (software IEEE binary128) and `float16` are the next step; see the card in `docs/plans/`.
