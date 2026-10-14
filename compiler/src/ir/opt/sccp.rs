//! Sparse conditional constant propagation: work out which values are constants and which blocks can
//! run at all, then fold. A branch on a known condition becomes a jump (and the other side is
//! dropped by the CFG pass), so a drop flag that is always set stops costing anything.

use crate::ir::*;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Lat {
    /// Nothing known yet (not reached).
    Top,
    Known(Val),
    /// Not a constant.
    Bot,
}

fn join(a: Lat, b: Lat) -> Lat {
    match (a, b) {
        (Lat::Top, x) | (x, Lat::Top) => x,
        (Lat::Known(x), Lat::Known(y)) if x == y => Lat::Known(x),
        _ => Lat::Bot,
    }
}

fn foldable(op: &Op) -> bool {
    matches!(
        op,
        Op::Const(..)
            | Op::Bin(..)
            | Op::Checked(..)
            | Op::Cmp(..)
            | Op::Neg(_)
            | Op::Not(_)
            | Op::Cast(..)
    )
}

pub fn run(f: &mut Func) -> bool {
    let n = f.blocks.len();
    let mut lat = vec![Lat::Top; f.values.len()];
    for p in f.blocks[0].params.clone() {
        lat[p.0 as usize] = Lat::Bot;
    }
    let mut exec = vec![false; n];
    exec[0] = true;
    loop {
        let mut changed = false;
        for bi in 0..n {
            if !exec[bi] {
                continue;
            }
            for &ii in &f.blocks[bi].insts {
                let inst = &f.insts[ii];
                let Some(r) = inst.result else { continue };
                let new = eval(f, &inst.op, &lat);
                let joined = join(lat[r.0 as usize], new);
                if joined != lat[r.0 as usize] {
                    lat[r.0 as usize] = joined;
                    changed = true;
                }
            }
            let Some(term) = f.blocks[bi].term.clone() else {
                continue;
            };
            let edges: Vec<&Target> = match &term {
                Term::Jump(t) => vec![t],
                Term::Branch(c, a, b) => match lat[c.0 as usize] {
                    Lat::Top => vec![],
                    Lat::Known(Val::Bool(true)) => vec![a],
                    Lat::Known(Val::Bool(false)) => vec![b],
                    _ => vec![a, b],
                },
                _ => vec![],
            };
            for t in edges {
                let to = t.block.0 as usize;
                if !exec[to] {
                    exec[to] = true;
                    changed = true;
                }
                let params = f.blocks[to].params.clone();
                for (p, a) in params.iter().zip(&t.args) {
                    let joined = join(lat[p.0 as usize], lat[a.0 as usize]);
                    if joined != lat[p.0 as usize] {
                        lat[p.0 as usize] = joined;
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    apply(f, &lat, &exec)
}

fn eval(f: &Func, op: &Op, lat: &[Lat]) -> Lat {
    if !foldable(op) {
        return Lat::Bot;
    }
    let mut any_top = false;
    let mut any_bot = false;
    op.for_each_value(|v| match lat[v.0 as usize] {
        Lat::Top => any_top = true,
        Lat::Bot => any_bot = true,
        Lat::Known(_) => {}
    });
    if any_bot {
        return Lat::Bot;
    }
    if any_top {
        return Lat::Top;
    }
    let get = |v: Value| match lat[v.0 as usize] {
        Lat::Known(x) => Some(x),
        _ => None,
    };
    match fold(op, f, &get) {
        Some(v) => Lat::Known(v),
        None => Lat::Bot,
    }
}

fn constant(v: Val) -> Option<Const> {
    match v {
        Val::Int(u) => Some(Const::Int(u as i128)),
        Val::Bool(b) => Some(Const::Bool(b)),
        _ => None,
    }
}

fn apply(f: &mut Func, lat: &[Lat], exec: &[bool]) -> bool {
    let mut changed = false;
    // Branches on a known condition.
    for bi in 0..f.blocks.len() {
        if !exec[bi] {
            continue;
        }
        if let Some(Term::Branch(c, a, b)) = f.blocks[bi].term.clone() {
            if let Lat::Known(Val::Bool(v)) = lat[c.0 as usize] {
                f.blocks[bi].term = Some(Term::Jump(if v { a } else { b }));
                changed = true;
            }
        }
    }
    // Instructions with a known result become constants.
    for bi in 0..f.blocks.len() {
        if !exec[bi] {
            continue;
        }
        for ii in f.blocks[bi].insts.clone() {
            let Some(r) = f.insts[ii].result else {
                continue;
            };
            if matches!(f.insts[ii].op, Op::Const(..)) {
                continue;
            }
            if let (Lat::Known(v), true) = (lat[r.0 as usize], foldable(&f.insts[ii].op)) {
                if let Some(c) = constant(v) {
                    f.insts[ii].op = Op::Const(f.ty(r), c);
                    changed = true;
                }
            }
        }
    }
    // Block parameters with a known value: the uses see a constant made at the top of the block.
    let mut map: HashMap<Value, Value> = HashMap::new();
    let uses = f.use_counts();
    for bi in 1..f.blocks.len() {
        if !exec[bi] {
            continue;
        }
        let mut at = 0;
        for p in f.blocks[bi].params.clone() {
            if let (Lat::Known(v), true) = (lat[p.0 as usize], uses[p.0 as usize] > 0) {
                if let Some(c) = constant(v) {
                    let ty = f.ty(p);
                    let nv = f.new_value(ty);
                    f.insts.push(Inst {
                        result: Some(nv),
                        op: Op::Const(ty, c),
                        span: crate::span::Span::new(0, 0),
                    });
                    let id = f.insts.len() - 1;
                    f.blocks[bi].insts.insert(at, id);
                    at += 1;
                    map.insert(p, nv);
                }
            }
        }
    }
    if !map.is_empty() {
        f.replace_uses(&map);
        changed = true;
    }
    if changed {
        f.compact();
    }
    changed
}
