//! Tail calls to the function itself become jumps (card A-11). `return f(x - 1, acc * x)` at the end
//! of `f` runs in constant stack and without a call: the arguments go to the parameters of a new
//! loop header and control jumps back to the top.
//!
//! The old entry block becomes the loop header (it keeps the parameters and the code); a new entry
//! block just jumps to it. Slots are made again on every pass through the header, which is what a
//! fresh call did, so the only thing to rule out is a call that was handed the address of one of
//! this frame's slots: the next round would overwrite what the callee was reading.

use crate::ir::*;
use std::collections::HashSet;

/// How the sum is formed in an accumulator site: `Bin(Add)` or `Checked(Add, signed)`.
#[derive(Clone, Copy, PartialEq)]
enum Sum {
    Wrapping,
    Checked(bool),
}

struct Site {
    block: usize,
    /// The arguments of the recursive call.
    args: Vec<Value>,
    /// For `return x + f(args)`: the kind of addition and `x`.
    sum: Option<(Sum, Value)>,
}

pub fn run(f: &mut Func, me: FuncId) -> bool {
    let mut sites = tail_calls(f, me);
    if sites.is_empty() || escapes(f, &sites) {
        return false;
    }
    // One kind of addition for the accumulator; a site with another kind stays an ordinary return.
    let kind = sites.iter().find_map(|s| s.sum.map(|(k, _)| k));
    sites.retain(|s| s.sum.is_none_or(|(k, _)| Some(k) == kind));
    let header = BlockId(f.blocks.len() as u32);
    // The entry block moves to `header`; a fresh entry block jumps to it.
    let old = std::mem::take(&mut f.blocks[0]);
    let fresh: Vec<Value> = f.params.clone().iter().map(|t| f.new_value(*t)).collect();
    let acc = kind.map(|_| f.new_value(f.ret));
    f.blocks.push(old);
    let mut entry_args = fresh.clone();
    if let Some(a) = acc {
        // The accumulator starts at zero, the identity of the sum; it is a parameter of the header.
        f.blocks[header.0 as usize].params.push(a);
        let zero = f
            .emit(BlockId(0), Op::Const(f.ret, Const::Int(0)), Some(f.ret))
            .unwrap();
        entry_args.push(zero);
    }
    f.blocks[0].params = fresh;
    f.blocks[0].term = Some(Term::Jump(Target {
        block: header,
        args: entry_args,
    }));
    for b in f.blocks.iter_mut().skip(1) {
        if let Some(t) = &mut b.term {
            for tg in t.targets_mut() {
                if tg.block == BlockId(0) {
                    tg.block = header;
                }
            }
        }
    }
    let site_blocks: HashSet<usize> = sites.iter().map(|s| s.block).collect();
    for s in sites {
        let bid = BlockId(s.block as u32);
        let mut args = s.args;
        f.blocks[s.block].insts.pop();
        if let (Some((sum, x)), Some(a)) = (s.sum, acc) {
            // acc + x, then the call that was added to it becomes the next round.
            f.blocks[s.block].insts.pop();
            let op = match sum {
                Sum::Wrapping => Op::Bin(BinOp::Add, a, x),
                Sum::Checked(signed) => Op::Checked(BinOp::Add, signed, a, x),
            };
            let next = f.emit(bid, op, Some(f.ret)).unwrap();
            args.push(next);
        } else if let Some(a) = acc {
            args.push(a);
        }
        f.blocks[s.block].term = Some(Term::Jump(Target {
            block: header,
            args,
        }));
    }
    // Every other way out of the function adds what was collected.
    if let (Some(a), Some(kind)) = (acc, kind) {
        for bi in 0..f.blocks.len() {
            if site_blocks.contains(&bi) || bi == 0 {
                continue;
            }
            if let Some(Term::Return(Some(v))) = f.blocks[bi].term {
                let op = match kind {
                    Sum::Wrapping => Op::Bin(BinOp::Add, a, v),
                    Sum::Checked(signed) => Op::Checked(BinOp::Add, signed, a, v),
                };
                let total = f.emit(BlockId(bi as u32), op, Some(f.ret)).unwrap();
                f.blocks[bi].term = Some(Term::Return(Some(total)));
            }
        }
    }
    true
}

/// The blocks that end in `return f(args)`, or in `return x + f(args)`, for the function itself.
fn tail_calls(f: &Func, me: FuncId) -> Vec<Site> {
    let uses = f.use_counts();
    let direct = |op: &Op| match op {
        Op::Call(callee, args) => {
            *callee == me
                && args.len() == f.params.len()
                && args.iter().zip(&f.params).all(|(a, t)| f.ty(*a) == *t)
        }
        _ => false,
    };
    let mut out = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        // Block 0 is the entry: a call there would be the whole body, and `return f(x)` forever.
        let (Some(&last), Some(r)) = (b.insts.last(), returned(f, b)) else {
            continue;
        };
        if bi == 0 {
            continue;
        }
        let inst = &f.insts[last];
        if let Op::Call(_, args) = &inst.op {
            let returns_it = match (inst.result, r) {
                (Some(v), Some(rv)) => v == rv && uses[v.0 as usize] == 1,
                (None, None) => true,
                _ => false,
            };
            if direct(&inst.op) && returns_it {
                out.push(Site {
                    block: bi,
                    args: args.clone(),
                    sum: None,
                });
            }
            continue;
        }
        // `x + f(args)` with the call right before the addition.
        let (sum, a, c) = match &inst.op {
            Op::Bin(BinOp::Add, a, c) => (Sum::Wrapping, *a, *c),
            Op::Checked(BinOp::Add, s, a, c) => (Sum::Checked(*s), *a, *c),
            _ => continue,
        };
        let (Some(total), Some(rv), Some(&prev)) = (inst.result, r, b.insts.iter().rev().nth(1))
        else {
            continue;
        };
        let call = &f.insts[prev];
        let (Some(cr), Op::Call(_, args)) = (call.result, &call.op) else {
            continue;
        };
        let x = if cr == c && cr != a {
            a
        } else if cr == a && cr != c {
            c
        } else {
            continue;
        };
        if direct(&call.op)
            && total == rv
            && uses[total.0 as usize] == 1
            && uses[cr.0 as usize] == 1
            && f.ty(cr) == f.ret
            && f.ret.is_int()
        {
            out.push(Site {
                block: bi,
                args: args.clone(),
                sum: Some((sum, x)),
            });
        }
    }
    out
}

/// What the block returns, looking through a jump to a block that only returns its parameter (the
/// join block of an `if` whose arms both end the function). `None` when it does not return.
fn returned(f: &Func, b: &Block) -> Option<Option<Value>> {
    match b.term.as_ref()? {
        Term::Return(r) => Some(*r),
        Term::Jump(t) => {
            let to = &f.blocks[t.block.0 as usize];
            match (&to.term, to.insts.is_empty()) {
                (Some(Term::Return(None)), true) => Some(None),
                (Some(Term::Return(Some(p))), true) => {
                    let k = to.params.iter().position(|q| q == p)?;
                    Some(Some(*t.args.get(k)?))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// Is the address of a slot of this frame passed to the call or stored somewhere?
fn escapes(f: &Func, sites: &[Site]) -> bool {
    let mut tainted: HashSet<Value> = HashSet::new();
    loop {
        let before = tainted.len();
        for b in &f.blocks {
            for &i in &b.insts {
                let (Some(r), op) = (f.insts[i].result, &f.insts[i].op) else {
                    continue;
                };
                match op {
                    Op::Slot(..) => {
                        tainted.insert(r);
                    }
                    Op::PtrAdd(a, _) if tainted.contains(a) => {
                        tainted.insert(r);
                    }
                    _ => {}
                }
            }
            if let Some(t) = &b.term {
                let mut edge = |tg: &Target| {
                    for (p, a) in f.block_params(tg.block).iter().zip(&tg.args) {
                        if tainted.contains(a) {
                            tainted.insert(*p);
                        }
                    }
                };
                match t {
                    Term::Jump(tg) => edge(tg),
                    Term::Branch(_, a, c) => {
                        edge(a);
                        edge(c);
                    }
                    _ => {}
                }
            }
        }
        if tainted.len() == before {
            break;
        }
    }
    if sites
        .iter()
        .any(|s| s.args.iter().any(|a| tainted.contains(a)))
    {
        return true;
    }
    f.blocks.iter().any(|b| {
        b.insts
            .iter()
            .any(|&i| matches!(&f.insts[i].op, Op::Store(_, v) if tainted.contains(v)))
    })
}
