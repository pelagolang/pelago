//! A negation in front of a multiplication or division by a constant (card A-11): `(-x) * c` is
//! `x * (-c)`, exactly (the sign of a product is the exclusive or of the signs, and negating a
//! constant changes no digit), so the sign flip leaves the chain of dependent operations. In a loop
//! like `a = -x * 0.001; v += a * 0.01` that chain is what the time is made of.

use crate::ir::*;

pub fn run(f: &mut Func) -> bool {
    let defs = f.defs();
    let mut edits: Vec<(usize, usize, usize, Value, IrTy, f64)> = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for (pos, &ii) in b.insts.iter().enumerate() {
            let Op::Bin(o @ (BinOp::Mul | BinOp::Div), a, c) = f.insts[ii].op else {
                continue;
            };
            let t = f.ty(a);
            if !matches!(t, IrTy::F32 | IrTy::F64) {
                continue;
            }
            let konst = |v: Value| match defs[v.0 as usize].map(|d| &f.insts[d].op) {
                Some(Op::Const(_, Const::Float(x))) => Some(*x),
                _ => None,
            };
            let neg = |v: Value| match defs[v.0 as usize].map(|d| &f.insts[d].op) {
                Some(Op::Neg(x)) => Some(*x),
                _ => None,
            };
            // `(-x) * c`, `c * (-x)` and `(-x) / c`; a constant divided by a negation is not this.
            let (x, k) = match (neg(a), konst(c), konst(a), neg(c)) {
                (Some(x), Some(k), _, _) => (x, k),
                (_, _, Some(k), Some(x)) if o == BinOp::Mul => (x, k),
                _ => continue,
            };
            edits.push((bi, pos, ii, x, t, k));
        }
    }
    if edits.is_empty() {
        return false;
    }
    // Later positions first, so the earlier ones stay where they are.
    edits.sort_by_key(|e| std::cmp::Reverse((e.0, e.1)));
    for (bi, pos, ii, x, t, k) in edits {
        let nk = f.new_value(t);
        let span = f.insts[ii].span;
        f.insts.push(Inst {
            result: Some(nk),
            op: Op::Const(t, Const::Float(-k)),
            span,
        });
        let id = f.insts.len() - 1;
        f.blocks[bi].insts.insert(pos, id);
        let Op::Bin(o, ..) = f.insts[ii].op else {
            unreachable!()
        };
        // For `c * (-x)` the product is commutative, so `x * (-c)` serves too.
        f.insts[ii].op = Op::Bin(o, x, nk);
    }
    true
}
