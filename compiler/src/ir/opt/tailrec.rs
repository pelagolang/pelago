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

pub fn run(f: &mut Func, me: FuncId) -> bool {
    let sites = tail_calls(f, me);
    if sites.is_empty() || escapes(f, &sites) {
        return false;
    }
    let header = BlockId(f.blocks.len() as u32);
    // The entry block moves to `header`; a fresh entry block jumps to it.
    let old = std::mem::take(&mut f.blocks[0]);
    let tys: Vec<IrTy> = f.params.clone();
    let fresh: Vec<Value> = tys.iter().map(|t| f.new_value(*t)).collect();
    f.blocks.push(old);
    f.blocks[0] = Block {
        params: fresh.clone(),
        insts: Vec::new(),
        term: Some(Term::Jump(Target {
            block: header,
            args: fresh,
        })),
    };
    for b in f.blocks.iter_mut().skip(1) {
        if let Some(t) = &mut b.term {
            for tg in t.targets_mut() {
                if tg.block == BlockId(0) {
                    tg.block = header;
                }
            }
        }
    }
    for (bi, args) in sites {
        let b = &mut f.blocks[bi];
        b.insts.pop();
        b.term = Some(Term::Jump(Target {
            block: header,
            args,
        }));
    }
    true
}

/// The blocks that end in `return f(args)` for the function itself, with the arguments.
fn tail_calls(f: &Func, me: FuncId) -> Vec<(usize, Vec<Value>)> {
    let uses = f.use_counts();
    let mut out = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        let (Some(&last), Some(r)) = (b.insts.last(), returned(f, b)) else {
            continue;
        };
        let inst = &f.insts[last];
        let Op::Call(callee, args) = &inst.op else {
            continue;
        };
        let direct = *callee == me
            && args.len() == f.params.len()
            && args.iter().zip(&f.params).all(|(a, t)| f.ty(*a) == *t);
        let returns_it = match (inst.result, r) {
            (Some(v), Some(rv)) => v == rv && uses[v.0 as usize] == 1,
            (None, None) => true,
            _ => false,
        };
        // Block 0 is the entry: a call there would be the whole body, and `return f(x)` forever.
        if direct && returns_it && bi != 0 {
            out.push((bi, args.clone()));
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
fn escapes(f: &Func, sites: &[(usize, Vec<Value>)]) -> bool {
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
        .any(|(_, args)| args.iter().any(|a| tainted.contains(a)))
    {
        return true;
    }
    f.blocks.iter().any(|b| {
        b.insts
            .iter()
            .any(|&i| matches!(&f.insts[i].op, Op::Store(_, v) if tainted.contains(v)))
    })
}
