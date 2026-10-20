//! Block order (card A-11, W9). The native code generator writes the blocks in the order they have
//! in the function and jumps unless the target is the next block, so the order decides how many
//! jumps run. This pass chooses it:
//!
//! * The first successor of a branch comes first (the `then` side, the body of a loop), the rest
//!   of the function after it.
//! * A loop is laid out with its test at the bottom: body first, header last. The loop is entered
//!   by one jump to the header, and after that every iteration ends in the header's conditional
//!   jump back to the body, instead of a conditional jump out plus an unconditional jump back.
//! * Blocks that only stop the program go to the end, out of the way of the code that runs.
//!
//! Nothing but the order of `Func::blocks` changes (and the block numbers in the jumps).

use super::dom::Dom;
use super::loops;
use crate::ir::*;

pub fn run(f: &mut Func) -> bool {
    let n = f.blocks.len();
    let dom = Dom::new(f);
    if dom.rpo.len() != n {
        return false;
    }
    let mut order = reverse_post_order(f);
    for lp in loops::find(f, &dom) {
        if lp.header.0 == 0 {
            continue;
        }
        let hp = order.iter().position(|b| *b == lp.header).unwrap();
        let last = order.iter().rposition(|b| lp.contains(*b)).unwrap();
        if last > hp {
            order.remove(hp);
            order.insert(last, lp.header);
        }
    }
    let cold = |b: &BlockId| {
        matches!(
            f.blocks[b.0 as usize].term,
            Some(Term::Stop(_) | Term::Unreachable)
        )
    };
    let (mut hot, cold): (Vec<BlockId>, Vec<BlockId>) =
        order.iter().partition(|b| !cold(b) || b.0 == 0);
    hot.extend(cold);
    if hot.iter().enumerate().all(|(i, b)| b.0 as usize == i) {
        return false;
    }
    let mut new_id = vec![0u32; n];
    for (i, b) in hot.iter().enumerate() {
        new_id[b.0 as usize] = i as u32;
    }
    let mut old: Vec<Option<Block>> = std::mem::take(&mut f.blocks)
        .into_iter()
        .map(Some)
        .collect();
    for b in &hot {
        let mut blk = old[b.0 as usize].take().unwrap();
        if let Some(t) = &mut blk.term {
            for tg in t.targets_mut() {
                tg.block = BlockId(new_id[tg.block.0 as usize]);
            }
        }
        f.blocks.push(blk);
    }
    true
}

/// Reverse post-order of a depth-first search that tries the last successor first, so that the
/// first successor comes first in the result.
fn reverse_post_order(f: &Func) -> Vec<BlockId> {
    let n = f.blocks.len();
    let mut seen = vec![false; n];
    let mut post = Vec::with_capacity(n);
    let succ = |b: BlockId| {
        let mut s = f.successors(b);
        s.reverse();
        s
    };
    let mut stack = vec![(BlockId(0), succ(BlockId(0)), 0usize)];
    seen[0] = true;
    while let Some((b, ss, i)) = stack.last_mut() {
        if let Some(&s) = ss.get(*i) {
            *i += 1;
            if !seen[s.0 as usize] {
                seen[s.0 as usize] = true;
                stack.push((s, succ(s), 0));
            }
        } else {
            post.push(*b);
            stack.pop();
        }
    }
    post.reverse();
    post
}
