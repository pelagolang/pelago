//! Dominators (Cooper, Harvey and Kennedy, "A Simple, Fast Dominance Algorithm"). Block `a`
//! dominates block `b` when every path from the entry to `b` goes through `a`; the immediate
//! dominator is the closest one. Used by the range analysis.

use crate::ir::*;

const NONE: u32 = u32::MAX;

pub struct Dom {
    /// Reachable blocks in reverse post-order (the entry first).
    pub rpo: Vec<BlockId>,
    /// Position of each block in `rpo`, `NONE` for unreachable blocks.
    pos: Vec<u32>,
    /// Immediate dominator of each block (`NONE` for the entry and for unreachable blocks).
    idom: Vec<u32>,
}

impl Dom {
    pub fn new(f: &Func) -> Dom {
        let n = f.blocks.len();
        // Post-order by an iterative depth-first search.
        let mut seen = vec![false; n];
        let mut post = Vec::with_capacity(n);
        let mut stack = vec![(0u32, f.successors(BlockId(0)), 0usize)];
        seen[0] = true;
        while let Some((b, succ, i)) = stack.last_mut() {
            if let Some(&s) = succ.get(*i) {
                *i += 1;
                if !seen[s.0 as usize] {
                    seen[s.0 as usize] = true;
                    stack.push((s.0, f.successors(s), 0));
                }
            } else {
                post.push(BlockId(*b));
                stack.pop();
            }
        }
        post.reverse();
        let rpo = post;
        let mut pos = vec![NONE; n];
        for (i, b) in rpo.iter().enumerate() {
            pos[b.0 as usize] = i as u32;
        }
        let preds = f.predecessors();
        let mut idom = vec![NONE; n];
        if let Some(e) = rpo.first() {
            idom[e.0 as usize] = e.0;
        }
        let mut changed = true;
        while changed {
            changed = false;
            for &b in rpo.iter().skip(1) {
                let mut new = NONE;
                for p in &preds[b.0 as usize] {
                    let p = p.0;
                    if pos[p as usize] == NONE || idom[p as usize] == NONE {
                        continue;
                    }
                    new = if new == NONE {
                        p
                    } else {
                        intersect(&idom, &pos, p, new)
                    };
                }
                if new != idom[b.0 as usize] {
                    idom[b.0 as usize] = new;
                    changed = true;
                }
            }
        }
        if let Some(e) = rpo.first() {
            idom[e.0 as usize] = NONE;
        }
        Dom { rpo, pos, idom }
    }

    pub fn reachable(&self, b: BlockId) -> bool {
        self.pos[b.0 as usize] != NONE
    }

    /// The closest strict dominator (`None` for the entry and unreachable blocks).
    pub fn idom(&self, b: BlockId) -> Option<BlockId> {
        match self.idom[b.0 as usize] {
            NONE => None,
            d => Some(BlockId(d)),
        }
    }
}

fn intersect(idom: &[u32], pos: &[u32], mut a: u32, mut b: u32) -> u32 {
    while a != b {
        while pos[a as usize] > pos[b as usize] {
            a = idom[a as usize];
        }
        while pos[b as usize] > pos[a as usize] {
            b = idom[b as usize];
        }
    }
    a
}
