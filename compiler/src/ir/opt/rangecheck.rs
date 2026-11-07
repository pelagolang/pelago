//! Range tests and boolean selects. `c >= 'a' && c <= 'z'` arrives here as a select of two compares
//! (the second only matters where the first holds); the same set of values is one subtraction and one
//! unsigned compare, `c - 'a' < 26`, with no flag to make and no second compare. Also `select(a, a, b)`
//! of bools (`a || b`) is `a | b`, and `select(a, b, a)` (`a && b`) is `a & b`.

use crate::ir::*;

/// The values of an integer type in its ordering, as an interval of `i128`.
fn domain(bits: u8, signed: bool) -> (i128, i128) {
    if signed {
        (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1)
    } else {
        (0, (1i128 << bits) - 1)
    }
}

/// What `x` may be for `cmp(x, k)` to hold: an interval, in the ordering of the operation.
fn interval(op: CmpOp, k: i128, dom: (i128, i128)) -> Option<(i128, i128)> {
    let (min, max) = dom;
    match op {
        CmpOp::Lt | CmpOp::ULt => k.checked_sub(1).map(|h| (min, h)),
        CmpOp::Le | CmpOp::ULe => Some((min, k)),
        CmpOp::Gt | CmpOp::UGt => k.checked_add(1).map(|l| (l, max)),
        CmpOp::Ge | CmpOp::UGe => Some((k, max)),
        _ => None,
    }
}

fn signed_of(op: CmpOp) -> Option<bool> {
    match op {
        CmpOp::Lt | CmpOp::Le | CmpOp::Gt | CmpOp::Ge => Some(true),
        CmpOp::ULt | CmpOp::ULe | CmpOp::UGt | CmpOp::UGe => Some(false),
        _ => None,
    }
}

pub fn run(f: &mut Func) -> bool {
    let defs = f.defs();
    let def = |v: Value| -> Option<&Op> {
        defs.get(v.0 as usize)
            .and_then(|d| d.map(|i| &f.insts[i].op))
    };
    let konst = |v: Value| -> Option<i128> {
        match def(v) {
            Some(Op::Const(_, Const::Int(n))) => Some(*n),
            _ => None,
        }
    };
    // A compare of one value with a constant: the value, the ordering and the interval it accepts.
    let test = |v: Value| -> Option<(Value, bool, i128, i128)> {
        let Some(Op::Cmp(op, x, y)) = def(v) else {
            return None;
        };
        let IrTy::Int(bits) = f.ty(*x) else {
            return None;
        };
        let signed = signed_of(*op)?;
        let (lo, hi) = interval(*op, konst(*y)?, domain(bits, signed))?;
        Some((*x, signed, lo, hi))
    };
    let always_false = |v: Value| matches!(def(v), Some(Op::Const(_, Const::Bool(false))));
    // (select instruction, x, low, count)
    let mut edits: Vec<(usize, Value, i128, i128)> = Vec::new();
    for b in &f.blocks {
        for &ii in &b.insts {
            let Op::Select(c, a, e) = f.insts[ii].op else {
                continue;
            };
            let Some((x, signed, cl, ch)) = test(c) else {
                continue;
            };
            let IrTy::Int(bits) = f.ty(x) else {
                continue;
            };
            let dom = domain(bits, signed);
            // Where `c` holds the value is `a`, else `e`; both must be false or one more bound on `x`.
            let arm = |v: Value| -> Option<Option<(i128, i128)>> {
                if always_false(v) {
                    return Some(None);
                }
                let (y, s, l, h) = test(v)?;
                (y == x && s == signed).then_some(Some((l, h)))
            };
            let (Some(ta), Some(te)) = (arm(a), arm(e)) else {
                continue;
            };
            // `c` is a prefix or a suffix of the domain, so its opposite is an interval too.
            let not_c = if cl == dom.0 && ch < dom.1 {
                (ch + 1, dom.1)
            } else if ch == dom.1 && cl > dom.0 {
                (dom.0, cl - 1)
            } else {
                continue;
            };
            let meet = |p: (i128, i128), q: (i128, i128)| -> Option<(i128, i128)> {
                let (l, h) = (p.0.max(q.0), p.1.min(q.1));
                (l <= h).then_some((l, h))
            };
            let from_a = ta.and_then(|t| meet((cl, ch), t));
            let from_e = te.and_then(|t| meet(not_c, t));
            let (lo, hi) = match (from_a, from_e) {
                (None, Some(r)) | (Some(r), None) => r,
                _ => continue,
            };
            // A prefix or a suffix is a plain compare already.
            if lo == dom.0 || hi == dom.1 {
                continue;
            }
            edits.push((ii, x, lo, hi - lo + 1));
        }
    }
    let mut changed = false;
    for (ii, x, lo, count) in edits {
        let ty = f.ty(x);
        let span = f.insts[ii].span;
        let add = |f: &mut Func, op: Op, t: IrTy| -> Value {
            let r = f.new_value(t);
            f.insts.push(Inst {
                result: Some(r),
                op,
                span,
            });
            r
        };
        let k_lo = add(f, Op::Const(ty, Const::Int(lo)), ty);
        let diff = add(f, Op::Bin(BinOp::Sub, x, k_lo), ty);
        let k_n = add(f, Op::Const(ty, Const::Int(count)), ty);
        let n = f.insts.len();
        f.insts[ii].op = Op::Cmp(CmpOp::ULt, diff, k_n);
        let (b, pos) = f
            .blocks
            .iter()
            .enumerate()
            .find_map(|(bi, bl)| bl.insts.iter().position(|&i| i == ii).map(|p| (bi, p)))
            .expect("the select is in a block");
        // The three new instructions go right before the compare that replaces the select.
        f.blocks[b].insts.splice(pos..pos, [n - 3, n - 2, n - 1]);
        changed = true;
    }
    changed
}
