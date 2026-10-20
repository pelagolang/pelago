//! If-conversion (card A-11): a branch whose two sides only compute a few values and then meet again
//! becomes straight-line code with a conditional move. A branch the processor cannot predict (the
//! parity test of `collatz`) costs a pipeline flush every time it is wrong; a `select` costs one cycle.
//!
//! ```text
//! h:  branch c, t, e          h:  v1 = ushr n, 1
//! t:  v1 = ushr n, 1    ->        _  = mul_checked_if not c, 3, n    (only the test)
//!     jump j(v1)                  v2 = mul 3, n
//! e:  v2 = mul_checked 3, n       v3 = select c, v1, v2
//!     jump j(v2)                  jump j(v3)
//! ```
//!
//! Both sides now run, so everything on them must be harmless to run when it is not wanted: no memory,
//! no calls, no division. A checked operation is the one that is not: it would stop the program for a
//! value the other side never lets it see. It becomes a `CheckedIf` that stops only when the side it
//! came from is the one taken (the overflow test is the same flag as before; the extra test of the
//! condition sits out of line), so the program stops exactly where it did. Values that differ between
//! the sides become selects of integers, booleans or addresses (not floats).

use crate::ir::*;
use crate::span::Span;

/// The most real instructions (constants are free) on one side, and on both sides together.
const SIDE: usize = 4;
const BOTH: usize = 6;

pub fn run(f: &mut Func) -> bool {
    let mut changed = false;
    // Each conversion changes the predecessor counts, so they are counted again after it.
    loop {
        let preds: Vec<usize> = f.predecessors().iter().map(|p| p.len()).collect();
        let mut done = false;
        for h in 0..f.blocks.len() {
            if try_block(f, h, &preds) {
                f.compact();
                done = true;
                break;
            }
        }
        if !done {
            return changed;
        }
        changed = true;
    }
}

struct Side {
    block: usize,
    insts: Vec<usize>,
    jump: Target,
}

/// A block that can run ahead of the test: reached only from `h`, no parameters, harmless
/// instructions, and it jumps on. `target` is how `h` gets there.
fn side(f: &Func, target: &Target, h: usize, preds: &[usize]) -> Option<Side> {
    let b = target.block.0 as usize;
    if b == h || preds[b] != 1 || !target.args.is_empty() || !f.blocks[b].params.is_empty() {
        return None;
    }
    let Some(Term::Jump(j)) = &f.blocks[b].term else {
        return None;
    };
    let mut real = 0;
    for &i in &f.blocks[b].insts {
        match &f.insts[i].op {
            Op::Const(..) => {}
            Op::Bin(o, ..) if !matches!(o, BinOp::Div | BinOp::Rem | BinOp::UDiv | BinOp::URem) => {
                real += 1
            }
            Op::Checked(..)
            | Op::Select(..)
            | Op::Cmp(..)
            | Op::Neg(_)
            | Op::Not(_)
            | Op::Cast(..)
            | Op::PtrAdd(..) => real += 1,
            _ => return None,
        }
    }
    (real <= SIDE).then(|| Side {
        block: b,
        insts: f.blocks[b].insts.clone(),
        jump: j.clone(),
    })
}

fn real_len(f: &Func, s: &Option<Side>) -> usize {
    s.as_ref().map_or(0, |s| {
        s.insts
            .iter()
            .filter(|&&i| !matches!(f.insts[i].op, Op::Const(..)))
            .count()
    })
}

fn args_differ(a: &[Value], b: &[Value]) -> bool {
    a.iter().zip(b).any(|(x, y)| x != y)
}

fn try_block(f: &mut Func, h: usize, preds: &[usize]) -> bool {
    let Some(Term::Branch(c, t, e)) = f.blocks[h].term.clone() else {
        return false;
    };
    if t.block == e.block {
        return false;
    }
    let (st, se) = (side(f, &t, h, preds), side(f, &e, h, preds));
    // The join and the arguments each way.
    let (join, targs, eargs, use_t, use_e) = match (&st, &se) {
        (Some(a), Some(b)) if a.jump.block == b.jump.block => (
            a.jump.block,
            a.jump.args.clone(),
            b.jump.args.clone(),
            true,
            true,
        ),
        (Some(a), _) if a.jump.block == e.block => {
            (e.block, a.jump.args.clone(), e.args.clone(), true, false)
        }
        (_, Some(b)) if b.jump.block == t.block => {
            (t.block, t.args.clone(), b.jump.args.clone(), false, true)
        }
        _ => return false,
    };
    let st = st.filter(|_| use_t);
    let se = se.filter(|_| use_e);
    if real_len(f, &st) + real_len(f, &se) > BOTH || join.0 as usize == h {
        return false;
    }
    if st.as_ref().is_some_and(|s| s.block == join.0 as usize)
        || se.as_ref().is_some_and(|s| s.block == join.0 as usize)
    {
        return false;
    }
    // Values that differ must be selectable.
    for (a, b) in targs.iter().zip(&eargs) {
        if a != b && matches!(f.ty(*a), IrTy::F32 | IrTy::F64) {
            return false;
        }
    }
    let span = Span::new(0, 0);
    let add = |f: &mut Func, op: Op, ty: IrTy| -> Value {
        let r = f.new_value(ty);
        f.insts.push(Inst {
            result: Some(r),
            op,
            span,
        });
        let id = f.insts.len() - 1;
        f.blocks[h].insts.push(id);
        r
    };
    // How a side tells that it is the one taken: the comparison that made the condition (or its
    // opposite) when there is one, else the bool.
    let cmp_def = f.blocks[h]
        .insts
        .iter()
        .find(|&&i| f.insts[i].result == Some(c))
        .and_then(|&i| match f.insts[i].op {
            Op::Cmp(o, x, y) if !matches!(f.ty(x), IrTy::F32 | IrTy::F64) => Some((o, x, y)),
            _ => None,
        });
    for (s, when) in [(&st, true), (&se, false)] {
        let test = match (cmp_def, when) {
            (Some((o, x, y)), true) => Test::Cmp(o, x, y),
            (Some((o, x, y)), false) => Test::Cmp(o.inverse(), x, y),
            (None, _) => Test::Bool(c, when),
        };
        let Some(s) = s else { continue };
        for &i in &s.insts {
            if let Op::Checked(o, signed, a, b) = f.insts[i].op.clone() {
                // The test stands alone and its result is unused; the value is the plain operation, so
                // the chain that feeds the select is as short as it can be.
                let ty = f.ty(a);
                add(f, Op::CheckedIf(o, signed, test, a, b), ty);
                f.insts[i].op = Op::Bin(o, a, b);
            }
            f.blocks[h].insts.push(i);
        }
        f.blocks[s.block].insts.clear();
        f.blocks[s.block].term = Some(Term::Unreachable);
    }
    // The selects read the opposite comparison made again right before them, so the flags it sets are
    // still there for their conditional moves (and nothing has to make the bool). It is a different
    // comparison, which keeps common-subexpression elimination from joining it with the first one.
    let mut cond = (c, false);
    if let Some((o, x, y)) = cmp_def {
        if args_differ(&targs, &eargs) {
            cond = (add(f, Op::Cmp(o.inverse(), x, y), IrTy::Bool), true);
        }
    }
    let mut args = Vec::new();
    for (a, b) in targs.iter().zip(&eargs) {
        if a == b {
            args.push(*a);
        } else {
            let ty = f.ty(*a);
            let op = if cond.1 {
                Op::Select(cond.0, *b, *a)
            } else {
                Op::Select(cond.0, *a, *b)
            };
            args.push(add(f, op, ty));
        }
    }
    f.blocks[h].term = Some(Term::Jump(Target { block: join, args }));
    true
}
