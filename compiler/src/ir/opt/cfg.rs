//! Simplify the control-flow graph: fold branches whose result is known without analysis, skip empty
//! blocks, merge a block into its only predecessor, drop blocks nobody can reach.

use crate::ir::*;
use std::collections::HashMap;

pub fn run(f: &mut Func) -> bool {
    let mut changed = false;
    loop {
        let mut round = false;
        round |= fold_branches(f);
        round |= skip_empty(f);
        round |= merge_blocks(f);
        let before = f.blocks.len();
        f.compact();
        round |= f.blocks.len() != before;
        if !round {
            return changed;
        }
        changed = true;
    }
}

/// `branch c, B(args), B(args)` is a jump; a branch on a constant is a jump too.
fn fold_branches(f: &mut Func) -> bool {
    let defs = f.defs();
    let mut changed = false;
    for bi in 0..f.blocks.len() {
        let Some(Term::Branch(c, a, b)) = f.blocks[bi].term.clone() else {
            continue;
        };
        let known = defs[c.0 as usize].and_then(|i| match f.insts[i].op {
            Op::Const(_, Const::Bool(v)) => Some(v),
            _ => None,
        });
        let pick = if a == b {
            Some(a)
        } else {
            known.map(|v| if v { a } else { b })
        };
        if let Some(t) = pick {
            f.blocks[bi].term = Some(Term::Jump(t));
            changed = true;
        }
    }
    changed
}

/// A block with no parameters and no instructions that only jumps on: its predecessors go straight
/// to its target.
fn skip_empty(f: &mut Func) -> bool {
    let mut changed = false;
    for bi in 1..f.blocks.len() {
        let b = &f.blocks[bi];
        if !b.params.is_empty() || !b.insts.is_empty() {
            continue;
        }
        let Some(Term::Jump(t)) = b.term.clone() else {
            continue;
        };
        if t.block == BlockId(bi as u32) {
            continue;
        }
        let me = BlockId(bi as u32);
        for pj in 0..f.blocks.len() {
            if pj == bi {
                continue;
            }
            if let Some(term) = &mut f.blocks[pj].term {
                for tg in term.targets_mut() {
                    if tg.block == me {
                        tg.block = t.block;
                        tg.args = t.args.clone();
                        changed = true;
                    }
                }
            }
        }
    }
    changed
}

/// `A: ... jump B(args)` where B has no other predecessor: B's code continues A.
fn merge_blocks(f: &mut Func) -> bool {
    let mut changed = false;
    loop {
        let preds = f.predecessors();
        let mut merged = false;
        for ai in 0..f.blocks.len() {
            let Some(Term::Jump(t)) = f.blocks[ai].term.clone() else {
                continue;
            };
            let bi = t.block.0 as usize;
            if bi == 0 || bi == ai || preds[bi].len() != 1 {
                continue;
            }
            let params = f.blocks[bi].params.clone();
            let map: HashMap<Value, Value> = params.into_iter().zip(t.args.iter().copied()).collect();
            let insts = std::mem::take(&mut f.blocks[bi].insts);
            let term = f.blocks[bi].term.take();
            f.blocks[bi].params.clear();
            f.blocks[ai].insts.extend(insts);
            f.blocks[ai].term = term;
            f.replace_uses(&map);
            merged = true;
            changed = true;
            break;
        }
        if !merged {
            return changed;
        }
    }
}
