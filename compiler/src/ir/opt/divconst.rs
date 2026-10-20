//! Division and remainder by a constant power of two (card A-11, step S1): `x / 2^k` and `x % 2^k`
//! become shifts, masks and a short fix-up for negative numbers, so the native code does not run
//! `idiv` (and its zero and `-1` checks) for them. Division by 1 needs no division either.
//!
//! Any other constant divisor becomes a high multiply and a shift (`MulHi` / `UMulHi`, the method of
//! Granlund and Montgomery and of Hacker's Delight, chapter 10), so `x / 10` is a multiply, a shift and
//! a sign fix-up. A remainder is `x - (x / c) * c`.
//!
//! The results are the same as the interpreter's: signed division rounds toward zero and the
//! remainder takes the sign of the dividend. The divisors 0, 1 and -1 and the negative powers of two
//! are left alone (zero must still stop the program, `MIN / -1` wraps).

use crate::ir::*;
use crate::span::Span;

/// The positive power of two a constant divisor stands for, as `k` with divisor `2^k`.
fn pow2(c: i128, ty: IrTy, signed: bool) -> Option<u32> {
    let IrTy::Int(bits) = ty else { return None };
    let bits = bits as u32;
    let v = if signed {
        // The constant is stored as written; `MIN` and the other negative divisors are not handled.
        if c <= 0 || c >= (1i128 << (bits - 1)) {
            return None;
        }
        c
    } else {
        if c <= 0 || c >= (1i128 << bits) {
            return None;
        }
        c
    };
    (v & (v - 1) == 0).then(|| v.trailing_zeros())
}

/// How to divide by an unsigned constant: `q = mulhi(n, m) >> shift`, or, when the multiplier needs
/// one bit more than the type has, `t = mulhi(n, m); q = (t + ((n - t) >> 1)) >> shift`.
#[derive(Debug, PartialEq, Eq)]
struct UMagic {
    m: u128,
    shift: u32,
    add: bool,
}

/// `d` is not a power of two and `1 < d < 2^bits`; `bits <= 64`.
fn umagic(d: u128, bits: u32) -> UMagic {
    let l = 128 - (d - 1).leading_zeros(); // ceil(log2 d)
                                           // The exact reciprocal `ceil(2^(bits+s) / d)` works for every `n < 2^bits` when its error is at most `2^s`.
    for s in 0..=l {
        let p = bits + s;
        if p >= 127 {
            break;
        }
        let m = (1u128 << p).div_ceil(d);
        let e = m * d - (1u128 << p);
        if m < (1u128 << bits) && e <= (1u128 << s) {
            return UMagic {
                m,
                shift: s,
                add: false,
            };
        }
    }
    let m = ((1u128 << bits) * ((1u128 << l) - d)) / d + 1;
    UMagic {
        m,
        shift: l - 1,
        add: true,
    }
}

/// Signed: `q = mulhi(n, m)`, plus `n` when `m` was made negative for a positive divisor, minus `n` for
/// the opposite case, then `>> shift`, then plus the sign bit of the result.
#[derive(Debug, PartialEq, Eq)]
struct SMagic {
    m: u128, // `bits` wide
    shift: u32,
}

/// Hacker's Delight `magic`. `d` is a signed divisor with `2 <= |d| < 2^(bits-1)`; `bits <= 64`.
fn smagic(d: i128, bits: u32) -> SMagic {
    let two = 1u128 << (bits - 1);
    let ad = d.unsigned_abs();
    let t = two + (d < 0) as u128;
    let anc = t - 1 - t % ad;
    let mut p = bits - 1;
    let (mut q1, mut r1) = (two / anc, two % anc);
    let (mut q2, mut r2) = (two / ad, two % ad);
    loop {
        p += 1;
        q1 *= 2;
        r1 *= 2;
        if r1 >= anc {
            q1 += 1;
            r1 -= anc;
        }
        q2 *= 2;
        r2 *= 2;
        if r2 >= ad {
            q2 += 1;
            r2 -= ad;
        }
        let delta = ad - r2;
        if !(q1 < delta || (q1 == delta && r1 == 0)) {
            break;
        }
    }
    let mut m = (q2 + 1) & ((1u128 << bits) - 1);
    if d < 0 {
        m = m.wrapping_neg() & ((1u128 << bits) - 1);
    }
    SMagic { m, shift: p - bits }
}

fn emit(f: &mut Func, out: &mut Vec<usize>, ty: IrTy, op: Op) -> Value {
    let r = f.new_value(ty);
    f.insts.push(Inst {
        result: Some(r),
        op,
        span: Span::new(0, 0),
    });
    out.push(f.insts.len() - 1);
    r
}

fn konst(f: &mut Func, out: &mut Vec<usize>, ty: IrTy, n: i128) -> Value {
    emit(f, out, ty, Op::Const(ty, Const::Int(n)))
}

/// `x % 2^k == 0` (or `!= 0`) only asks whether the low `k` bits are clear, whatever the sign of `x`,
/// so the comparison takes `x & (2^k - 1)` and needs no fix-up at all. Runs before the rewrite below.
fn rem_compare(f: &mut Func, defs: &[Option<usize>]) -> bool {
    let mut changed = false;
    for ii in 0..f.insts.len() {
        let Op::Cmp(c @ (CmpOp::Eq | CmpOp::Ne), r, z) = f.insts[ii].op.clone() else {
            continue;
        };
        let konst_of = |v: Value| match defs[v.0 as usize].map(|d| &f.insts[d].op) {
            Some(Op::Const(_, Const::Int(n))) => Some(*n),
            _ => None,
        };
        if konst_of(z) != Some(0) {
            continue;
        }
        let Some(rd) = defs[r.0 as usize] else {
            continue;
        };
        let Op::Bin(BinOp::Rem | BinOp::URem, x, d) = f.insts[rd].op.clone() else {
            continue;
        };
        let signed = matches!(f.insts[rd].op, Op::Bin(BinOp::Rem, ..));
        let ty = f.ty(x);
        let Some(k) = konst_of(d).and_then(|n| pow2(n, ty, signed)) else {
            continue;
        };
        if k == 0 {
            continue;
        }
        // `r` itself stays (it may have other users); the comparison reads a mask of `x` instead.
        let m = f.new_value(ty);
        f.insts.push(Inst {
            result: Some(m),
            op: Op::Const(ty, Const::Int((1i128 << k) - 1)),
            span: Span::new(0, 0),
        });
        let mc = f.insts.len() - 1;
        let a = f.new_value(ty);
        f.insts.push(Inst {
            result: Some(a),
            op: Op::Bin(BinOp::And, x, m),
            span: Span::new(0, 0),
        });
        let ac = f.insts.len() - 1;
        // Put both just before the comparison, in its block.
        for b in 0..f.blocks.len() {
            if let Some(pos) = f.blocks[b].insts.iter().position(|i| *i == ii) {
                f.blocks[b].insts.splice(pos..pos, [mc, ac]);
                break;
            }
        }
        f.insts[ii].op = Op::Cmp(c, a, z);
        changed = true;
    }
    changed
}

/// Rewrites the division or remainder at `ii` by the constant `n` (as written in the IR) into a high
/// multiply. Pushes the new instructions and `ii` itself onto `out`; `false` when the divisor is not
/// one it handles (nothing was pushed then).
#[allow(clippy::too_many_arguments)]
fn magic(
    f: &mut Func,
    out: &mut Vec<usize>,
    o: BinOp,
    a: Value,
    n: i128,
    ty: IrTy,
    bits: u32,
    ii: usize,
) -> bool {
    let signed = matches!(o, BinOp::Div | BinOp::Rem);
    let is_rem = matches!(o, BinOp::Rem | BinOp::URem);
    let wide = 1i128 << bits;
    let full = (1u128 << bits) - 1;
    // Signed constants may be written negative or as their unsigned bit pattern.
    let d = if signed && n >= wide / 2 && n < wide {
        n - wide
    } else {
        n
    };
    let start = out.len();
    let q = if signed {
        let ad = d.unsigned_abs();
        // 0, 1, -1 and the negative powers of two (`MIN` included) stay as they are.
        if ad < 2 || ad >= 1u128 << (bits - 1) || (d < 0 && ad.is_power_of_two()) {
            return false;
        }
        let sm = smagic(d, bits);
        let mc = konst(f, out, ty, sm.m as i128);
        let mut q = emit(f, out, ty, Op::Bin(BinOp::MulHi, a, mc));
        let m_neg = sm.m >> (bits - 1) != 0;
        if d > 0 && m_neg {
            q = emit(f, out, ty, Op::Bin(BinOp::Add, q, a));
        } else if d < 0 && !m_neg {
            q = emit(f, out, ty, Op::Bin(BinOp::Sub, q, a));
        }
        if sm.shift > 0 {
            let s = konst(f, out, ty, sm.shift as i128);
            q = emit(f, out, ty, Op::Bin(BinOp::Shr, q, s));
        }
        let w = konst(f, out, ty, bits as i128 - 1);
        let sign = emit(f, out, ty, Op::Bin(BinOp::UShr, q, w));
        emit(f, out, ty, Op::Bin(BinOp::Add, q, sign))
    } else {
        if n < 2 || n as u128 > full {
            return false;
        }
        let um = umagic(n as u128, bits);
        let mc = konst(f, out, ty, um.m as i128);
        let mut q = emit(f, out, ty, Op::Bin(BinOp::UMulHi, a, mc));
        if um.add {
            let diff = emit(f, out, ty, Op::Bin(BinOp::Sub, a, q));
            let one = konst(f, out, ty, 1);
            let half = emit(f, out, ty, Op::Bin(BinOp::UShr, diff, one));
            q = emit(f, out, ty, Op::Bin(BinOp::Add, q, half));
        }
        if um.shift > 0 {
            let s = konst(f, out, ty, um.shift as i128);
            q = emit(f, out, ty, Op::Bin(BinOp::UShr, q, s));
        }
        q
    };
    let last = if is_rem {
        let dc = konst(f, out, ty, n);
        let prod = emit(f, out, ty, Op::Bin(BinOp::Mul, q, dc));
        Op::Bin(BinOp::Sub, a, prod)
    } else {
        // The quotient is already in `q`; `ii` becomes a copy of it.
        Op::Bin(BinOp::Or, q, q)
    };
    debug_assert!(out.len() > start);
    f.insts[ii].op = last;
    out.push(ii);
    true
}

pub fn run(f: &mut Func) -> bool {
    let defs = f.defs();
    let mut changed = rem_compare(f, &defs);
    for bi in 0..f.blocks.len() {
        let order = f.blocks[bi].insts.clone();
        let mut out: Vec<usize> = Vec::with_capacity(order.len());
        for ii in order {
            let Op::Bin(o, a, b) = f.insts[ii].op.clone() else {
                out.push(ii);
                continue;
            };
            let signed = matches!(o, BinOp::Div | BinOp::Rem);
            if !signed && !matches!(o, BinOp::UDiv | BinOp::URem) {
                out.push(ii);
                continue;
            }
            let ty = f.ty(a);
            let Some(Op::Const(_, Const::Int(n))) =
                defs[b.0 as usize].map(|d| f.insts[d].op.clone())
            else {
                out.push(ii);
                continue;
            };
            let IrTy::Int(bits) = ty else { unreachable!() };
            let bits = bits as u32;
            let span = f.insts[ii].span;
            let is_rem = matches!(o, BinOp::Rem | BinOp::URem);
            let Some(k) = pow2(n, ty, signed) else {
                if bits <= 64 && magic(f, &mut out, o, a, n, ty, bits, ii) {
                    changed = true;
                } else {
                    out.push(ii);
                }
                continue;
            };
            let last = if k == 0 {
                // Divide by one: the quotient is `a`, the remainder is 0.
                if is_rem {
                    Op::Const(ty, Const::Int(0))
                } else {
                    Op::Bin(BinOp::Or, a, a)
                }
            } else if !signed {
                if is_rem {
                    let m = konst(f, &mut out, ty, (1i128 << k) - 1);
                    Op::Bin(BinOp::And, a, m)
                } else {
                    let s = konst(f, &mut out, ty, k as i128);
                    Op::Bin(BinOp::UShr, a, s)
                }
            } else {
                // q = (a + ((a >> (bits-1)) >>> (bits-k))) >> k : negative dividends get 2^k - 1 added first.
                let w1 = konst(f, &mut out, ty, bits as i128 - 1);
                let sign = emit(f, &mut out, ty, Op::Bin(BinOp::Shr, a, w1));
                let w2 = konst(f, &mut out, ty, (bits - k) as i128);
                let bias = emit(f, &mut out, ty, Op::Bin(BinOp::UShr, sign, w2));
                let t = emit(f, &mut out, ty, Op::Bin(BinOp::Add, a, bias));
                let sk = konst(f, &mut out, ty, k as i128);
                if is_rem {
                    // r = a - ((t >> k) << k) = a - (t with the low k bits cleared)
                    let mask = konst(f, &mut out, ty, !((1i128 << k) - 1));
                    let hi = emit(f, &mut out, ty, Op::Bin(BinOp::And, t, mask));
                    Op::Bin(BinOp::Sub, a, hi)
                } else {
                    Op::Bin(BinOp::Shr, t, sk)
                }
            };
            f.insts[ii].op = last;
            f.insts[ii].span = span;
            out.push(ii);
            changed = true;
        }
        f.blocks[bi].insts = out;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the plan for an unsigned divisor in `bits`-bit arithmetic.
    fn udiv(n: u128, d: u128, bits: u32) -> u128 {
        let um = umagic(d, bits);
        assert!(um.m < 1 << bits);
        let t = (n * um.m) >> bits;
        let q = if um.add { t + ((n - t) >> 1) } else { t };
        q >> um.shift
    }

    fn sdiv(n: i128, d: i128, bits: u32) -> i128 {
        let sm = smagic(d, bits);
        let m = sm.m as i128;
        let m = if m >> (bits - 1) != 0 {
            m - (1 << bits)
        } else {
            m
        };
        let mut q = (n * m) >> bits;
        if d > 0 && m < 0 {
            q += n;
        } else if d < 0 && m > 0 {
            q -= n;
        }
        q >>= sm.shift;
        q + ((q >> (bits - 1)) & 1) // the sign bit of the (wrapped) quotient
    }

    #[test]
    fn unsigned_magic_is_exact_for_all_8_and_16_bit_cases() {
        for d in (3u128..256).filter(|d| !d.is_power_of_two()) {
            for n in 0u128..256 {
                assert_eq!(udiv(n, d, 8), n / d, "{n} / {d}");
            }
        }
        for d in (3u128..65536).filter(|d| !d.is_power_of_two()).step_by(97) {
            for n in (0u128..65536).step_by(13).chain([65535, 65534]) {
                assert_eq!(udiv(n, d, 16), n / d, "{n} / {d}");
            }
        }
    }

    #[test]
    fn signed_magic_is_exact_for_all_8_bit_cases() {
        for d in (-127i128..=127)
            .filter(|d| d.abs() >= 2 && !(*d < 0 && d.unsigned_abs().is_power_of_two()))
        {
            for n in -128i128..=127 {
                assert_eq!(sdiv(n, d, 8), n / d, "{n} / {d}");
            }
        }
    }

    #[test]
    fn wide_magic_is_exact_at_the_edges() {
        for bits in [32u32, 64] {
            let max = (1u128 << bits) - 1;
            for d in [
                3u128,
                5,
                6,
                7,
                9,
                10,
                11,
                100,
                641,
                1000,
                12345,
                max / 3,
                max / 2 - 1,
                max / 2 + 1,
                max - 1,
                max,
            ] {
                if d.is_power_of_two() {
                    continue;
                }
                for n in [
                    0u128,
                    1,
                    2,
                    d - 1,
                    d,
                    d + 1,
                    2 * d - 1,
                    max / 2,
                    max - 1,
                    max,
                    max / d * d,
                    max / d * d - 1,
                ] {
                    assert_eq!(
                        udiv(n.min(max), d, bits),
                        n.min(max) / d,
                        "{bits}: {n} / {d}"
                    );
                }
            }
            let (lo, hi) = (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1);
            for d in [
                3i128,
                5,
                7,
                10,
                11,
                100,
                641,
                1000,
                12345,
                hi,
                hi - 1,
                -3,
                -7,
                -10,
                -641,
                lo + 1,
                lo + 2,
            ] {
                if d < 0 && d.unsigned_abs().is_power_of_two() || d.unsigned_abs().is_power_of_two()
                {
                    continue;
                }
                for n in [
                    lo,
                    lo + 1,
                    -d - 1,
                    -d,
                    -d + 1,
                    -1,
                    0,
                    1,
                    d - 1,
                    d,
                    d + 1,
                    hi - 1,
                    hi,
                    hi / d * d,
                    hi / d * d + 1,
                ] {
                    if !(lo..=hi).contains(&n) {
                        continue;
                    }
                    assert_eq!(sdiv(n, d, bits), n / d, "{bits}: {n} / {d}");
                }
            }
        }
    }
}
