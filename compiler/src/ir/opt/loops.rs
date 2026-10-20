//! Natural loops: a back edge `u -> h` where `h` dominates `u` makes a loop with header `h`; its body
//! is `h` and everything that reaches `u` without passing `h`. Back edges to one header are one loop.

use super::dom::Dom;
use crate::ir::*;

pub struct Loop {
    pub header: BlockId,
    /// `body[b]` is true for the blocks of the loop (the header included).
    pub body: Vec<bool>,
    pub size: usize,
}

/// The loops of a function, innermost (smallest) first.
pub fn find(f: &Func, dom: &Dom) -> Vec<Loop> {
    let preds = f.predecessors();
    let n = f.blocks.len();
    let mut loops: Vec<Loop> = Vec::new();
    for &h in &dom.rpo {
        let latches: Vec<BlockId> = preds[h.0 as usize]
            .iter()
            .copied()
            .filter(|p| dom.dominates(h, *p))
            .collect();
        if latches.is_empty() {
            continue;
        }
        let mut body = vec![false; n];
        body[h.0 as usize] = true;
        let mut stack = latches;
        while let Some(b) = stack.pop() {
            if !body[b.0 as usize] {
                body[b.0 as usize] = true;
                stack.extend(preds[b.0 as usize].iter().copied());
            }
        }
        let size = body.iter().filter(|x| **x).count();
        loops.push(Loop {
            header: h,
            body,
            size,
        });
    }
    loops.sort_by_key(|l| l.size);
    loops
}

impl Loop {
    pub fn contains(&self, b: BlockId) -> bool {
        self.body[b.0 as usize]
    }
}
