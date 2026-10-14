//! Counters for the report: how much code a function or module has.

use crate::ir::*;

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Stats {
    pub funcs: usize,
    pub blocks: usize,
    pub insts: usize,
    pub slots: usize,
    pub loads: usize,
    pub stores: usize,
    pub calls: usize,
}

pub fn stats_of(m: &Module) -> Stats {
    let mut s = Stats {
        funcs: m.funcs.len(),
        ..Stats::default()
    };
    for f in &m.funcs {
        s.blocks += f.blocks.len();
        for b in &f.blocks {
            for &i in &b.insts {
                s.insts += 1;
                match f.insts[i].op {
                    Op::Slot(..) => s.slots += 1,
                    Op::Load(..) => s.loads += 1,
                    Op::Store(..) => s.stores += 1,
                    Op::Call(..) | Op::CallPtr(..) => s.calls += 1,
                    _ => {}
                }
            }
        }
    }
    s
}
