//! Dead code: values nobody reads, stores into slots nobody reads, block parameters nobody reads.

use crate::ir::*;
use std::collections::HashSet;

pub fn run(f: &mut Func) -> bool {
    let mut changed = false;
    loop {
        let mut round = false;
        round |= dead_stores(f);
        round |= dead_insts(f);
        round |= dead_params(f);
        if !round {
            return changed;
        }
        changed = true;
    }
}

fn remove_insts(f: &mut Func, dead: &HashSet<usize>) {
    if dead.is_empty() {
        return;
    }
    for b in &mut f.blocks {
        b.insts.retain(|i| !dead.contains(i));
    }
}

/// Instructions that make a value nobody reads and change nothing.
fn dead_insts(f: &mut Func) -> bool {
    let mut any = false;
    loop {
        let uses = f.use_counts();
        let mut dead = HashSet::new();
        for b in &f.blocks {
            for &i in &b.insts {
                let inst = &f.insts[i];
                if let Some(r) = inst.result {
                    if uses[r.0 as usize] == 0 && inst.op.is_pure() {
                        dead.insert(i);
                    }
                }
            }
        }
        if dead.is_empty() {
            return any;
        }
        remove_insts(f, &dead);
        any = true;
    }
}

/// A slot that is only ever written: the stores and the slot go.
fn dead_stores(f: &mut Func) -> bool {
    let mut slots: Vec<Value> = Vec::new();
    for b in &f.blocks {
        for &i in &b.insts {
            if let (Op::Slot(..), Some(r)) = (&f.insts[i].op, f.insts[i].result) {
                slots.push(r);
            }
        }
    }
    if slots.is_empty() {
        return false;
    }
    let mut only_stored: HashSet<Value> = slots.iter().copied().collect();
    // Any use other than as the address of a store keeps the slot alive.
    for b in &f.blocks {
        for &i in &b.insts {
            match &f.insts[i].op {
                Op::Store(a, v) => {
                    // Storing the address itself somewhere lets it escape.
                    if only_stored.contains(v) {
                        only_stored.remove(v);
                    }
                    let _ = a;
                }
                op => op.for_each_value(|v| {
                    only_stored.remove(&v);
                }),
            }
        }
        if let Some(t) = &b.term {
            t.for_each_value(|v| {
                only_stored.remove(&v);
            });
        }
    }
    if only_stored.is_empty() {
        return false;
    }
    let mut dead = HashSet::new();
    for b in &f.blocks {
        for &i in &b.insts {
            match &f.insts[i].op {
                Op::Store(a, _) if only_stored.contains(a) => {
                    dead.insert(i);
                }
                Op::Slot(..) if f.insts[i].result.is_some_and(|r| only_stored.contains(&r)) => {
                    dead.insert(i);
                }
                _ => {}
            }
        }
    }
    remove_insts(f, &dead);
    !dead.is_empty()
}

/// Block parameters nobody reads: dropped together with the argument every jump passes for them.
/// A parameter only counts as read when something that matters reads it, so a counter that feeds
/// nothing but its own next value goes too.
fn dead_params(f: &mut Func) -> bool {
    let defs = f.defs();
    let mut param_of: Vec<Option<(usize, usize)>> = vec![None; f.values.len()];
    for (bi, b) in f.blocks.iter().enumerate() {
        for (i, p) in b.params.iter().enumerate() {
            param_of[p.0 as usize] = Some((bi, i));
        }
    }
    let mut live = vec![false; f.values.len()];
    let mut work: Vec<Value> = Vec::new();
    let mark = |v: Value, live: &mut Vec<bool>, work: &mut Vec<Value>| {
        if !live[v.0 as usize] {
            live[v.0 as usize] = true;
            work.push(v);
        }
    };
    for b in &f.blocks {
        for &i in &b.insts {
            let inst = &f.insts[i];
            if inst.result.is_none() || !inst.op.is_pure() {
                inst.op.for_each_value(|v| mark(v, &mut live, &mut work));
            }
        }
        match &b.term {
            Some(Term::Branch(c, ..)) => mark(*c, &mut live, &mut work),
            Some(Term::Return(Some(v))) => mark(*v, &mut live, &mut work),
            _ => {}
        }
    }
    while let Some(v) = work.pop() {
        if let Some(i) = defs[v.0 as usize] {
            f.insts[i]
                .op
                .for_each_value(|x| mark(x, &mut live, &mut work));
        } else if let Some((bi, idx)) = param_of[v.0 as usize] {
            for b in &f.blocks {
                match &b.term {
                    Some(Term::Jump(t)) if t.block.0 as usize == bi => {
                        mark(t.args[idx], &mut live, &mut work)
                    }
                    Some(Term::Branch(_, t, e)) => {
                        for tg in [t, e] {
                            if tg.block.0 as usize == bi {
                                mark(tg.args[idx], &mut live, &mut work);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    let mut drop_at: Vec<(usize, Vec<usize>)> = Vec::new();
    for bi in 1..f.blocks.len() {
        let idx: Vec<usize> = f.blocks[bi]
            .params
            .iter()
            .enumerate()
            .filter(|(_, p)| !live[p.0 as usize])
            .map(|(i, _)| i)
            .collect();
        if !idx.is_empty() {
            drop_at.push((bi, idx));
        }
    }
    if drop_at.is_empty() {
        return false;
    }
    for (bi, idx) in &drop_at {
        let keep = |i: usize| !idx.contains(&i);
        let mut k = 0;
        f.blocks[*bi].params.retain(|_| {
            let r = keep(k);
            k += 1;
            r
        });
        for b in &mut f.blocks {
            if let Some(t) = &mut b.term {
                for tg in t.targets_mut() {
                    if tg.block.0 as usize == *bi {
                        let mut k = 0;
                        tg.args.retain(|_| {
                            let r = keep(k);
                            k += 1;
                            r
                        });
                    }
                }
            }
        }
    }
    true
}
