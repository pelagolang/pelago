//! Division and remainder by a constant power of two (card A-11, step S1): `x / 2^k` and `x % 2^k`
//! become shifts, masks and a short fix-up for negative numbers, so the native code does not run
//! `idiv` (and its zero and `-1` checks) for them. Division by 1 needs no division either.
//!
//! The results are the same as the interpreter's: signed division rounds toward zero and the
//! remainder takes the sign of the dividend. Divisors that are not powers of two are left alone (they
//! need a high multiply, which the IR does not have yet).

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
            let Some(k) = pow2(n, ty, signed) else {
                out.push(ii);
                continue;
            };
            let IrTy::Int(bits) = ty else { unreachable!() };
            let bits = bits as u32;
            let span = f.insts[ii].span;
            let is_rem = matches!(o, BinOp::Rem | BinOp::URem);
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
