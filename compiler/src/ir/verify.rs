//! Checks the rules of the IR, so a mistake in lowering is reported as an IR error and not as a crash
//! in generated code.
//!
//! - every block has one terminator, and every block can be reached from the entry;
//! - a value is made once, and every use is of a value made earlier: in the same block above it, or in
//!   a block that dominates this one;
//! - operand types fit the operation; jumps hand the right number and types of values to the
//!   block parameters; calls match the callee's signature; `return` matches the function's type.

use super::*;
use std::collections::HashMap;

pub fn verify_module(m: &Module) -> Vec<String> {
    let mut out = Vec::new();
    for f in &m.funcs {
        out.extend(check(f, Some(m)));
    }
    out
}

pub fn verify_func(f: &Func) -> Vec<String> {
    check(f, None)
}

/// Where a value is made: a block, and a position in it (parameters come before position 0).
#[derive(Clone, Copy)]
struct Def {
    block: usize,
    pos: usize,
}

fn check(f: &Func, m: Option<&Module>) -> Vec<String> {
    let mut errs = Vec::new();
    macro_rules! err {
        ($m:expr) => {
            errs.push(format!("{}: {}", f.name, $m))
        };
    }

    // Where each value is made.
    let mut defs: HashMap<Value, Def> = HashMap::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for p in &b.params {
            if defs.insert(*p, Def { block: bi, pos: 0 }).is_some() {
                err!(format!("v{} is made twice", p.0));
            }
        }
        for (k, &id) in b.insts.iter().enumerate() {
            if let Some(r) = f.insts[id].result {
                if defs
                    .insert(
                        r,
                        Def {
                            block: bi,
                            pos: k + 1,
                        },
                    )
                    .is_some()
                {
                    err!(format!("v{} is made twice", r.0));
                }
            }
        }
    }
    if f.blocks.is_empty() {
        err!("has no blocks".to_string());
        return errs;
    }
    let ptys: Vec<IrTy> = f.blocks[0].params.iter().map(|p| f.ty(*p)).collect();
    if ptys != f.params {
        err!("the entry block's parameters do not match the function's".to_string());
    }

    let idom = dominators(f);

    let dominates = |a: usize, b: usize| -> bool {
        // Does block `a` dominate block `b`?
        let mut cur = b;
        loop {
            if cur == a {
                return true;
            }
            match idom.get(&cur) {
                Some(&p) if p != cur => cur = p,
                _ => return false,
            }
        }
    };

    for (bi, b) in f.blocks.iter().enumerate() {
        if !idom.contains_key(&bi) {
            err!(format!("b{bi} cannot be reached from the entry"));
        }
        let used = |pos: usize, x: Value, what: &str, errs2: &mut Vec<String>| match defs.get(&x) {
            None => errs2.push(format!(
                "{}: b{bi}: {what} uses v{}, which is never made",
                f.name, x.0
            )),
            Some(d) => {
                let ok = if d.block == bi {
                    d.pos <= pos
                } else {
                    idom.contains_key(&bi) && dominates(d.block, bi)
                };
                if !ok {
                    errs2.push(format!(
                        "{}: b{bi}: {what} uses v{} before it is made",
                        f.name, x.0
                    ));
                }
            }
        };
        let mut more: Vec<String> = Vec::new();
        for (k, &id) in b.insts.iter().enumerate() {
            let inst = &f.insts[id];
            let pos = k + 1;
            let name = format!("instruction {}", k + 1);
            for x in inputs(&inst.op) {
                used(pos, x, &name, &mut more);
            }
            for e in check_inst(f, m, inst) {
                more.push(format!("{}: b{bi}: {name}: {e}", f.name));
            }
        }
        let end = b.insts.len() + 1;
        match &b.term {
            None => more.push(format!("{}: b{bi} has no terminator", f.name)),
            Some(t) => {
                for x in term_inputs(t) {
                    used(end, x, "the terminator", &mut more);
                }
                for e in check_term(f, t) {
                    more.push(format!("{}: b{bi}: {e}", f.name));
                }
            }
        }
        errs.extend(more);
    }
    errs
}

/// Immediate dominators by the iterative algorithm of Cooper, Harvey and Kennedy.
/// Blocks that cannot be reached are left out.
fn dominators(f: &Func) -> HashMap<usize, usize> {
    let n = f.blocks.len();
    // Reverse postorder from the entry.
    let mut order: Vec<usize> = Vec::new();
    let mut seen = vec![false; n];
    let mut stack: Vec<(usize, usize)> = vec![(0, 0)];
    seen[0] = true;
    while let Some((b, i)) = stack.pop() {
        let succ = f.successors(BlockId(b as u32));
        if i < succ.len() {
            stack.push((b, i + 1));
            let s = succ[i].0 as usize;
            if s < n && !seen[s] {
                seen[s] = true;
                stack.push((s, 0));
            }
        } else {
            order.push(b);
        }
    }
    order.reverse();
    let mut rank = vec![usize::MAX; n];
    for (i, b) in order.iter().enumerate() {
        rank[*b] = i;
    }
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &b in &order {
        for s in f.successors(BlockId(b as u32)) {
            if (s.0 as usize) < n {
                preds[s.0 as usize].push(b);
            }
        }
    }
    let mut idom: HashMap<usize, usize> = HashMap::new();
    idom.insert(0, 0);
    let mut changed = true;
    while changed {
        changed = false;
        for &b in order.iter().skip(1) {
            let mut new: Option<usize> = None;
            for &p in &preds[b] {
                if !idom.contains_key(&p) {
                    continue;
                }
                new = Some(match new {
                    None => p,
                    Some(cur) => {
                        let (mut x, mut y) = (p, cur);
                        while x != y {
                            while rank[x] > rank[y] {
                                x = idom[&x];
                            }
                            while rank[y] > rank[x] {
                                y = idom[&y];
                            }
                        }
                        x
                    }
                });
            }
            if let Some(nd) = new {
                if idom.get(&b) != Some(&nd) {
                    idom.insert(b, nd);
                    changed = true;
                }
            }
        }
    }
    idom
}

fn inputs(op: &Op) -> Vec<Value> {
    match op {
        Op::Const(..) | Op::Slot(..) | Op::DataAddr(_) | Op::FuncAddr(_) => Vec::new(),
        Op::Bin(_, a, b)
        | Op::Checked(_, _, a, b)
        | Op::Cmp(_, a, b)
        | Op::VBin(_, a, b)
        | Op::Store(a, b)
        | Op::PtrAdd(a, b)
        | Op::Realloc(a, b)
        | Op::Print(a, b) => {
            vec![*a, *b]
        }
        Op::Neg(a)
        | Op::Not(a)
        | Op::VUn(_, a)
        | Op::Cast(_, a, _)
        | Op::Load(_, a)
        | Op::Alloc(a)
        | Op::Free(a) => {
            vec![*a]
        }
        Op::Call(_, args) => args.clone(),
        Op::CheckedIf(_, _, test, a, b) => {
            let mut v = Vec::new();
            test.for_each_value(|x| v.push(x));
            v.extend([*a, *b]);
            v
        }
        Op::Select(a, b, c) | Op::Fma(a, b, c) | Op::MemCopy(a, b, c) => vec![*a, *b, *c],
        Op::CallPtr(_, _, f, args) => {
            let mut v = vec![*f];
            v.extend(args.iter().copied());
            v
        }
    }
}

fn term_inputs(t: &Term) -> Vec<Value> {
    match t {
        Term::Jump(t) => t.args.clone(),
        Term::Branch(c, a, b) => {
            let mut v = vec![*c];
            v.extend(a.args.iter().copied());
            v.extend(b.args.iter().copied());
            v
        }
        Term::Return(Some(x)) => vec![*x],
        _ => Vec::new(),
    }
}

fn ty_of(f: &Func, x: Value) -> Option<IrTy> {
    f.values.get(x.0 as usize).copied()
}

fn check_inst(f: &Func, m: Option<&Module>, inst: &Inst) -> Vec<String> {
    let mut e = Vec::new();
    let t = |x: Value| ty_of(f, x);
    let result_ty = inst.result.and_then(|r| ty_of(f, r));
    match &inst.op {
        Op::Const(ty, c) => {
            let ok = match c {
                Const::Int(_) => ty.is_int(),
                Const::Float(_) => ty.is_float(),
                Const::Bool(_) => *ty == IrTy::Bool,
            };
            if !ok {
                e.push(format!("a constant does not fit the type {}", ty.name()));
            }
            if result_ty != Some(*ty) {
                e.push("the result type differs from the constant's type".to_string());
            }
        }
        Op::Bin(op, a, b) | Op::Checked(op, _, a, b) => {
            if t(*a) != t(*b) {
                e.push(format!("`{}` needs operands of one type", op.name()));
            } else if let Some(ty) = t(*a) {
                let fine = if op.is_bitwise() {
                    ty.is_int()
                } else {
                    ty.is_int() || (ty.is_float() && matches!(inst.op, Op::Bin(..)))
                };
                if !fine {
                    e.push(format!("`{}` does not work on {}", op.name(), ty.name()));
                }
                if result_ty != Some(ty) {
                    e.push("the result type differs from the operands'".to_string());
                }
            }
            if matches!(op, BinOp::MulHi | BinOp::UMulHi)
                && !matches!(t(*a), Some(IrTy::Int(1..=64)))
            {
                e.push("`mulhi` works on integers of 64 bits or fewer".to_string());
            }
            if matches!(inst.op, Op::Checked(..))
                && !matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul)
            {
                e.push("only add, sub and mul have checked forms".to_string());
            }
        }
        Op::CheckedIf(op, _, test, a, b) => {
            match test {
                Test::Bool(c, _) if t(*c) != Some(IrTy::Bool) => {
                    e.push("`checked_if` needs a bool condition".to_string());
                }
                Test::Cmp(_, x, y) if t(*x) != t(*y) => {
                    e.push("a comparison needs operands of one type".to_string());
                }
                _ => {}
            }
            if t(*a) != t(*b) || !t(*a).is_some_and(|x| x.is_int()) || result_ty != t(*a) {
                e.push("`checked_if` needs two integers of the result's type".to_string());
            }
            if !matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul) {
                e.push("only add, sub and mul have checked forms".to_string());
            }
        }
        Op::Select(c, a, b) => {
            if t(*c) != Some(IrTy::Bool) {
                e.push("`select` needs a bool condition".to_string());
            }
            if t(*a) != t(*b) || result_ty != t(*a) {
                e.push("`select` needs two values of the result's type".to_string());
            }
        }
        Op::Fma(a, b, c) => {
            if t(*a) != t(*b)
                || t(*a) != t(*c)
                || result_ty != t(*a)
                || !t(*a).is_some_and(|x| x.is_float())
            {
                e.push("`fma` needs three floats of the result's type".to_string());
            }
        }
        Op::Cmp(_, a, b) => {
            if t(*a) != t(*b) {
                e.push("a comparison needs operands of one type".to_string());
            }
            if result_ty != Some(IrTy::Bool) {
                e.push("a comparison makes a bool".to_string());
            }
        }
        Op::Neg(a) => {
            if !t(*a).is_some_and(|x| x.is_int() || x.is_float()) {
                e.push("`neg` needs a number".to_string());
            }
        }
        Op::Not(a) => {
            if !t(*a).is_some_and(|x| x.is_int() || x == IrTy::Bool) {
                e.push("`not` needs an integer or a bool".to_string());
            }
        }
        Op::VUn(o, a) => {
            let (from, to) = match o {
                VUn::Splat => (IrTy::I64, IrTy::V2I64),
                VUn::Shl(k) | VUn::UShr(k) => {
                    if *k >= 64 {
                        e.push("a vector shift count is below 64".to_string());
                    }
                    (IrTy::V2I64, IrTy::V2I64)
                }
                VUn::ReduceAdd => (IrTy::V2I64, IrTy::I64),
            };
            if t(*a) != Some(from) || result_ty != Some(to) {
                e.push(format!(
                    "a vector operation takes {} and makes {}",
                    from.name(),
                    to.name()
                ));
            }
        }
        Op::VBin(o, a, b) => {
            let (from, to) = match o {
                VBin::Build => (IrTy::I64, IrTy::V2I64),
                _ => (IrTy::V2I64, IrTy::V2I64),
            };
            if t(*a) != Some(from) || t(*b) != Some(from) || result_ty != Some(to) {
                e.push(format!(
                    "`{}` takes two {} and makes {}",
                    o.name(),
                    from.name(),
                    to.name()
                ));
            }
        }
        Op::Cast(kind, a, to) => {
            let from = t(*a);
            let ok = match (kind, from) {
                (CastKind::ZExt | CastKind::SExt, Some(IrTy::Int(x))) => {
                    matches!(to, IrTy::Int(y) if *y > x)
                }
                (CastKind::Trunc, Some(IrTy::Int(x))) => matches!(to, IrTy::Int(y) if *y < x),
                (CastKind::IntToFloat, Some(IrTy::Int(_))) => to.is_float(),
                (CastKind::FloatToInt, Some(x)) => x.is_float() && to.is_int(),
                (CastKind::FloatResize, Some(x)) => x.is_float() && to.is_float() && x != *to,
                (CastKind::PtrToInt, Some(IrTy::Ptr)) => *to == IrTy::I64,
                (CastKind::IntToPtr, Some(IrTy::Int(64))) => *to == IrTy::Ptr,
                _ => false,
            };
            if !ok {
                e.push(format!("`{}` does not fit these types", kind.name()));
            }
            if result_ty != Some(*to) {
                e.push("the result type differs from the cast's target".to_string());
            }
        }
        Op::Call(callee, args) => {
            if let Some(g) = m.and_then(|m| m.funcs.get(callee.0 as usize)) {
                if g.params.len() != args.len() {
                    e.push(format!(
                        "`{}` takes {} arguments, but {} were given",
                        g.name,
                        g.params.len(),
                        args.len()
                    ));
                } else {
                    for (i, (a, want)) in args.iter().zip(g.params.iter()).enumerate() {
                        if t(*a) != Some(*want) {
                            e.push(format!(
                                "argument {} of `{}` has the wrong type",
                                i + 1,
                                g.name
                            ));
                        }
                    }
                }
                let want = if g.ret == IrTy::Void {
                    None
                } else {
                    Some(g.ret)
                };
                if result_ty != want {
                    e.push(format!("the result type differs from `{}`'s", g.name));
                }
            } else if m.is_some() {
                e.push("calls a function that does not exist".to_string());
            }
        }
        Op::Slot(..) | Op::DataAddr(_) | Op::FuncAddr(_) => {
            if result_ty != Some(IrTy::Ptr) {
                e.push("this makes a ptr".to_string());
            }
        }
        Op::PtrAdd(p, n) => {
            if t(*p) != Some(IrTy::Ptr) || t(*n) != Some(IrTy::I64) {
                e.push("`ptradd` needs a ptr and an i64".to_string());
            }
            if result_ty != Some(IrTy::Ptr) {
                e.push("`ptradd` makes a ptr".to_string());
            }
        }
        Op::MemCopy(d, s, n) => {
            if t(*d) != Some(IrTy::Ptr) || t(*s) != Some(IrTy::Ptr) || t(*n) != Some(IrTy::I64) {
                e.push("`memcopy` needs two ptrs and an i64 size".to_string());
            }
        }
        Op::CallPtr(ps, ret, f, args) => {
            if t(*f) != Some(IrTy::Ptr) {
                e.push("`callptr` needs a function address".to_string());
            }
            if ps.len() != args.len() {
                e.push("`callptr` has the wrong number of arguments".to_string());
            } else {
                for (a, want) in args.iter().zip(ps.iter()) {
                    if t(*a) != Some(*want) {
                        e.push("an argument of `callptr` has the wrong type".to_string());
                    }
                }
            }
            let want = if *ret == IrTy::Void { None } else { Some(*ret) };
            if result_ty != want {
                e.push("the result type differs from the signature's".to_string());
            }
        }
        Op::Print(p, n) => {
            if t(*p) != Some(IrTy::Ptr) || t(*n) != Some(IrTy::I64) {
                e.push("`print` needs a ptr and an i64 length".to_string());
            }
        }
        Op::Load(ty, p) => {
            if t(*p) != Some(IrTy::Ptr) {
                e.push("`load` needs a ptr".to_string());
            }
            if result_ty != Some(*ty) {
                e.push("the result type differs from the loaded type".to_string());
            }
        }
        Op::Store(p, x) => {
            if t(*p) != Some(IrTy::Ptr) {
                e.push("`store` needs a ptr".to_string());
            }
            if t(*x).is_none() || t(*x) == Some(IrTy::Void) {
                e.push("`store` needs a value".to_string());
            }
            if inst.result.is_some() {
                e.push("`store` makes no value".to_string());
            }
        }
        Op::Alloc(n) => {
            if t(*n) != Some(IrTy::I64) {
                e.push("`alloc` needs an i64 size".to_string());
            }
            if result_ty != Some(IrTy::Ptr) {
                e.push("`alloc` makes a ptr".to_string());
            }
        }
        Op::Free(p) => {
            if t(*p) != Some(IrTy::Ptr) {
                e.push("`free` needs a ptr".to_string());
            }
        }
        Op::Realloc(p, n) => {
            if t(*p) != Some(IrTy::Ptr) || t(*n) != Some(IrTy::I64) {
                e.push("`realloc` needs a ptr and an i64 size".to_string());
            }
            if result_ty != Some(IrTy::Ptr) {
                e.push("`realloc` makes a ptr".to_string());
            }
        }
    }
    e
}

fn check_term(f: &Func, term: &Term) -> Vec<String> {
    let mut e = Vec::new();
    let n = f.blocks.len();
    let target = |t: &Target, e: &mut Vec<String>| {
        if t.block.0 as usize >= n {
            e.push(format!("jumps to b{}, which does not exist", t.block.0));
            return;
        }
        let params = &f.blocks[t.block.0 as usize].params;
        if params.len() != t.args.len() {
            e.push(format!(
                "b{} takes {} values, but {} were given",
                t.block.0,
                params.len(),
                t.args.len()
            ));
            return;
        }
        for (i, (a, p)) in t.args.iter().zip(params.iter()).enumerate() {
            if ty_of(f, *a) != ty_of(f, *p) {
                e.push(format!(
                    "value {} handed to b{} has the wrong type",
                    i + 1,
                    t.block.0
                ));
            }
        }
    };
    match term {
        Term::Jump(t) => target(t, &mut e),
        Term::Branch(c, a, b) => {
            if ty_of(f, *c) != Some(IrTy::Bool) {
                e.push("`branch` needs a bool".to_string());
            }
            target(a, &mut e);
            target(b, &mut e);
        }
        Term::Return(x) => match (x, f.ret) {
            (None, IrTy::Void) => {}
            (None, _) => e.push(format!("`return` needs a {} value", f.ret.name())),
            (Some(_), IrTy::Void) => e.push("this function returns nothing".to_string()),
            (Some(x), want) => {
                if ty_of(f, *x) != Some(want) {
                    e.push(format!(
                        "`return` has the wrong type; expected {}",
                        want.name()
                    ));
                }
            }
        },
        Term::Stop(_) | Term::Unreachable => {}
    }
    e
}
