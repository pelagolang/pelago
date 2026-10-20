//! Base-case peeling (card A-11). A recursive function that starts with a test and returns at once
//! on one side (`fib`: `if n < 2 { n }`) is called mostly with arguments that take that exit. The
//! test moves to the call: `f(a)` becomes `if test(a) { value } else { f(a) }`, so the call, with
//! its saves and restores of registers, happens only when the function has real work to do.
//!
//! `analyze` reads the exit off the function as the front end wrote it (before the tail-call pass
//! changes the entry), `apply` rewrites the calls to the function from its own body.

use crate::ir::*;
use std::collections::HashMap;

/// What a call may skip.
#[derive(Clone)]
pub struct Exit {
    /// The instructions of the entry block, to be repeated on the arguments. Pure and cannot stop.
    insts: Vec<(Op, Option<Value>)>,
    params: Vec<Value>,
    cond: Value,
    /// The branch goes to the exit when the condition is this.
    when: bool,
    value: Leaf,
}

#[derive(Clone)]
enum Leaf {
    Param(usize),
    Const(IrTy, Const),
}

/// How much work the repeated test may be.
const MAX_TEST: usize = 4;

pub fn analyze(f: &Func) -> Option<Exit> {
    let entry = &f.blocks[0];
    let Some(Term::Branch(c, t, e)) = &entry.term else {
        return None;
    };
    if entry.insts.len() > MAX_TEST {
        return None;
    }
    for &i in &entry.insts {
        let ok = match &f.insts[i].op {
            Op::Const(..) | Op::Cmp(..) | Op::Neg(_) | Op::Not(_) | Op::Cast(..) => true,
            Op::Bin(op, ..) => !matches!(
                op,
                BinOp::Div
                    | BinOp::Rem
                    | BinOp::UDiv
                    | BinOp::URem
                    | BinOp::Shl
                    | BinOp::Shr
                    | BinOp::UShr
            ),
            _ => false,
        };
        if !ok {
            return None;
        }
    }
    if !entry.insts.iter().any(|&i| f.insts[i].result == Some(*c)) {
        return None;
    }
    // An entry value that is a constant made there.
    let entry_const = |v: Value| {
        entry
            .insts
            .iter()
            .find_map(|&i| match (&f.insts[i].op, f.insts[i].result) {
                (Op::Const(ty, k), Some(r)) if r == v => Some((*ty, *k)),
                _ => None,
            })
    };
    for (side, other, when) in [(t, e, true), (e, t, false)] {
        if side.block == other.block {
            continue;
        }
        let x = &f.blocks[side.block.0 as usize];
        let Some(Term::Return(Some(v))) = &x.term else {
            continue;
        };
        // The exit block does nothing but return (a parameter of its own, or an entry value).
        if !x.insts.is_empty() {
            continue;
        }
        let from_entry = match x.params.iter().position(|p| p == v) {
            Some(j) => side.args.get(j).copied(),
            None => side.args.is_empty().then_some(*v),
        };
        let Some(from_entry) = from_entry else {
            continue;
        };
        let value = if let Some(k) = entry.params.iter().position(|p| *p == from_entry) {
            Leaf::Param(k)
        } else if let Some((ty, k)) = entry_const(from_entry) {
            Leaf::Const(ty, k)
        } else {
            continue;
        };
        // The other side must not need anything from here either (no arguments are repeated).
        return Some(Exit {
            insts: entry
                .insts
                .iter()
                .map(|&i| (f.insts[i].op.clone(), f.insts[i].result))
                .collect(),
            params: entry.params.clone(),
            cond: *c,
            when,
            value,
        });
    }
    None
}

/// Guards the calls of `me` in `f` that do not end the function. Returns whether it changed `f`.
pub fn apply(f: &mut Func, me: FuncId, exit: &Exit) -> bool {
    let sites: Vec<usize> = f
        .blocks
        .iter()
        .filter(|b| !matches!(b.term, Some(Term::Return(_))))
        .flat_map(|b| b.insts.iter().copied())
        .filter(|&i| {
            matches!(&f.insts[i].op, Op::Call(c, a) if *c == me && a.len() == exit.params.len())
                && f.insts[i].result.is_some()
        })
        .collect();
    let mut changed = false;
    for id in sites {
        let Some((bi, pos)) = f
            .blocks
            .iter()
            .enumerate()
            .find_map(|(bi, b)| b.insts.iter().position(|&i| i == id).map(|pos| (bi, pos)))
        else {
            continue;
        };
        let Op::Call(_, args) = f.insts[id].op.clone() else {
            continue;
        };
        let res = f.insts[id].result.unwrap();
        let ty = f.ty(res);
        // The code after the call, and the block that receives the result.
        let after: Vec<usize> = f.blocks[bi].insts.split_off(pos + 1);
        f.blocks[bi].insts.pop();
        let term = f.blocks[bi].term.take();
        let join = f.new_block(&[]);
        f.blocks[join.0 as usize].params.push(res);
        f.blocks[join.0 as usize].insts = after;
        f.blocks[join.0 as usize].term = term;
        // The test on the arguments.
        let mut map: HashMap<Value, Value> = exit
            .params
            .iter()
            .copied()
            .zip(args.iter().copied())
            .collect();
        for (op, r) in &exit.insts {
            let mut op = op.clone();
            op.map_values(|v| *map.get(&v).unwrap_or(&v));
            let made = f.emit(BlockId(bi as u32), op, r.map(|r| f.ty(r)));
            if let (Some(r), Some(made)) = (r, made) {
                map.insert(*r, made);
            }
        }
        let c = map[&exit.cond];
        let leave = f.new_block(&[]);
        let value = match &exit.value {
            Leaf::Param(k) => args[*k],
            Leaf::Const(t, k) => f.emit(leave, Op::Const(*t, *k), Some(*t)).unwrap(),
        };
        let to = |block| Target {
            block,
            args: vec![],
        };
        f.terminate(
            leave,
            Term::Jump(Target {
                block: join,
                args: vec![value],
            }),
        );
        // The call stays, in its own block, with a fresh result.
        let call = f.new_block(&[]);
        let fresh = f.new_value(ty);
        f.insts[id].result = Some(fresh);
        f.blocks[call.0 as usize].insts.push(id);
        f.terminate(
            call,
            Term::Jump(Target {
                block: join,
                args: vec![fresh],
            }),
        );
        let (yes, no) = if exit.when {
            (leave, call)
        } else {
            (call, leave)
        };
        f.terminate(BlockId(bi as u32), Term::Branch(c, to(yes), to(no)));
        changed = true;
    }
    changed
}
