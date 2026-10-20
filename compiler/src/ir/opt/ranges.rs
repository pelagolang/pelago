//! Overflow checks that cannot fire (card A-11, step S2). A forward range analysis finds values that
//! are known to be small non-negative numbers (a widened byte, `x & 255`, `x % 10`, `x >> 3`, a sum
//! or product of such values); a checked `add`, `sub` or `mul` whose result provably fits becomes the
//! plain operation, so the native code does not test the overflow flag.
//!
//! Ranges are `(lo, hi)` with `0 <= lo <= hi <= MAX` of the signed type, so the same range is true
//! for a signed and for an unsigned reading of the bits. Block parameters and loads are not tracked
//! (no range), so loops are left alone; this is the safe part of the analysis.

use crate::ir::*;

type Range = (u128, u128);

fn bits(t: IrTy) -> u32 {
    match t {
        IrTy::Int(b) => b as u32,
        _ => 64,
    }
}

fn smax(t: IrTy) -> u128 {
    (1u128 << (bits(t) - 1)) - 1
}

struct Ctx<'a> {
    f: &'a Func,
    defs: Vec<Option<usize>>,
    memo: Vec<Option<Option<Range>>>,
}

impl Ctx<'_> {
    fn range(&mut self, v: Value) -> Option<Range> {
        if let Some(r) = self.memo[v.0 as usize] {
            return r;
        }
        // A cycle can only go through a block parameter, which has no definition here.
        let r = self.defs[v.0 as usize].and_then(|d| self.of_op(v, &self.f.insts[d].op.clone()));
        self.memo[v.0 as usize] = Some(r);
        r
    }

    fn konst(&self, v: Value) -> Option<u128> {
        let d = self.defs[v.0 as usize]?;
        match self.f.insts[d].op {
            Op::Const(t, Const::Int(n)) if n >= 0 && bits(t) <= 64 => {
                let c = n as u128 & ((1u128 << bits(t)) - 1);
                (c <= smax(t)).then_some(c)
            }
            _ => None,
        }
    }

    fn of_op(&mut self, v: Value, op: &Op) -> Option<Range> {
        let t = self.f.ty(v);
        if !matches!(t, IrTy::Int(_)) {
            return None;
        }
        let max = smax(t);
        match op {
            Op::Const(..) => self.konst(v).map(|c| (c, c)),
            Op::Cast(CastKind::ZExt, a, _) => {
                let fb = bits(self.f.ty(*a));
                match self.range(*a) {
                    Some(r) => Some(r),
                    None if fb < bits(t) => Some((0, (1u128 << fb) - 1)),
                    None => None,
                }
            }
            Op::Bin(BinOp::And, a, b) => {
                let (ra, rb) = (self.range(*a), self.range(*b));
                match (ra, rb) {
                    (Some(x), Some(y)) => Some((0, x.1.min(y.1))),
                    (Some(x), None) | (None, Some(x)) => Some((0, x.1)),
                    _ => None,
                }
            }
            Op::Bin(BinOp::URem, _, b) => {
                let c = self.konst(*b).filter(|c| *c > 0)?;
                Some((0, c - 1))
            }
            Op::Bin(BinOp::Rem, a, b) => {
                let c = self.konst(*b).filter(|c| *c > 0)?;
                self.range(*a)?;
                Some((0, c - 1))
            }
            Op::Bin(BinOp::UShr, _, k) => {
                let k = self
                    .konst(*k)
                    .filter(|k| (1..bits(t) as u128).contains(k))?;
                Some((0, (1u128 << (bits(t) - k as u32)) - 1))
            }
            Op::Bin(BinOp::Shr, a, k) => {
                let k = self.konst(*k).filter(|k| *k < bits(t) as u128)?;
                let r = self.range(*a)?;
                Some((r.0 >> k, r.1 >> k))
            }
            Op::Bin(BinOp::Div | BinOp::UDiv, a, b) => {
                let c = self.konst(*b).filter(|c| *c > 0)?;
                let r = self.range(*a)?;
                Some((r.0 / c, r.1 / c))
            }
            Op::Bin(o @ (BinOp::Add | BinOp::Mul), a, b)
            | Op::Checked(o @ (BinOp::Add | BinOp::Mul), _, a, b) => {
                let (x, y) = (self.range(*a)?, self.range(*b)?);
                let (lo, hi) = if *o == BinOp::Add {
                    (x.0 + y.0, x.1 + y.1)
                } else {
                    (x.0.checked_mul(y.0)?, x.1.checked_mul(y.1)?)
                };
                (hi <= max).then_some((lo, hi))
            }
            _ => None,
        }
    }
}

pub fn run(f: &mut Func) -> bool {
    let mut cx = Ctx {
        f,
        defs: f.defs(),
        memo: vec![None; f.values.len()],
    };
    let mut safe = Vec::new();
    for ii in 0..f.insts.len() {
        let Op::Checked(o, signed, a, b) = f.insts[ii].op.clone() else {
            continue;
        };
        let Some(r) = f.insts[ii].result else {
            continue;
        };
        let t = f.ty(r);
        if !matches!(t, IrTy::Int(_)) {
            continue;
        }
        let (Some(x), Some(y)) = (cx.range(a), cx.range(b)) else {
            continue;
        };
        let fits = match o {
            BinOp::Add => x.1 + y.1 <= smax(t),
            BinOp::Mul => x.1.checked_mul(y.1).is_some_and(|p| p <= smax(t)),
            // Both in [0, MAX]: the difference is in [-MAX, MAX], which a signed type holds;
            // an unsigned one needs `a >= b`.
            _ => signed || x.0 >= y.1,
        };
        if fits {
            safe.push(ii);
        }
    }
    let changed = !safe.is_empty();
    for ii in safe {
        if let Op::Checked(o, _, a, b) = f.insts[ii].op.clone() {
            f.insts[ii].op = Op::Bin(o, a, b);
        }
    }
    changed
}
