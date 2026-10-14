//! The IR optimiser (card A-04, `docs/design/ir.md` IR23).
//!
//! The lowering is simple on purpose: every variable is a stack slot, every owner has a drop flag in a
//! slot, small library functions are real calls. These passes clean that up. Every pass works on one
//! [`Func`] (inlining on the whole module), keeps the IR valid, and keeps what the program does:
//! output, result, and the interpreter's leak, double-free and use-after-free checks.
//!
//! How to add a pass: write `fn run(f: &mut Func) -> bool` in its own file (true when it changed
//! something), call it from [`optimize_func`] in the right place, and add unit tests on hand-written
//! IR next to it. The differential tests (`lower/tests.rs`, `opt/tests.rs`) run every program with and
//! without the pipeline and compare the results, so a wrong pass shows up there. In debug and test
//! builds the verifier runs after every pass.

mod cfg;
mod dce;
mod inline;
mod promote;
mod sccp;
mod split;
mod stats;
#[cfg(test)]
mod tests;

pub use stats::{stats_of, Stats};

use super::*;

/// What the pipeline may do.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// 0: nothing. 1: the passes inside functions. 2: also inlining.
    pub level: u8,
}

impl Default for Options {
    fn default() -> Options {
        Options { level: 2 }
    }
}

/// Optimises a whole module in place.
pub fn optimize(m: &mut Module, opts: Options) {
    if opts.level == 0 {
        return;
    }
    for i in 0..m.funcs.len() {
        optimize_func(&mut m.funcs[i]);
    }
    if opts.level >= 2 && inline::run(m) {
        for i in 0..m.funcs.len() {
            optimize_func(&mut m.funcs[i]);
        }
    }
    check_module(m, "the pipeline");
}

/// The passes that work inside one function, repeated until nothing changes (at most a few rounds).
pub fn optimize_func(f: &mut Func) {
    for _ in 0..6 {
        let mut changed = false;
        changed |= step(f, "simplify", cfg::run);
        changed |= step(f, "dce", dce::run);
        changed |= step(f, "promote", promote::run);
        changed |= step(f, "split", split::run);
        changed |= step(f, "sccp", sccp::run);
        changed |= step(f, "simplify", cfg::run);
        changed |= step(f, "dce", dce::run);
        if !changed {
            break;
        }
    }
}

fn step(f: &mut Func, name: &str, pass: fn(&mut Func) -> bool) -> bool {
    let changed = pass(f);
    #[cfg(debug_assertions)]
    if changed {
        let errs = verify_func(f);
        assert!(
            errs.is_empty(),
            "pass `{name}` broke `{}`: {errs:?}\n{}",
            f.name,
            print_func(f, None)
        );
    }
    let _ = name;
    changed
}

#[cfg(debug_assertions)]
fn check_module(m: &Module, what: &str) {
    let errs = verify_module(m);
    assert!(errs.is_empty(), "{what} broke the module: {errs:?}");
}

#[cfg(not(debug_assertions))]
fn check_module(_m: &Module, _what: &str) {}

// ---- Shared helpers ------------------------------------------------------------------------------------

impl Op {
    /// Calls `f` on every value the operation reads.
    pub fn for_each_value(&self, mut f: impl FnMut(Value)) {
        match self {
            Op::Const(..) | Op::Slot(..) | Op::DataAddr(_) | Op::FuncAddr(_) => {}
            Op::Bin(_, a, b)
            | Op::Checked(_, _, a, b)
            | Op::Cmp(_, a, b)
            | Op::PtrAdd(a, b)
            | Op::Print(a, b)
            | Op::Store(a, b) => {
                f(*a);
                f(*b);
            }
            Op::Neg(a) | Op::Not(a) | Op::Cast(_, a, _) | Op::Load(_, a) | Op::Alloc(a) | Op::Free(a) => {
                f(*a)
            }
            Op::MemCopy(a, b, c) => {
                f(*a);
                f(*b);
                f(*c);
            }
            Op::Call(_, args) => args.iter().for_each(|v| f(*v)),
            Op::CallPtr(_, _, p, args) => {
                f(*p);
                args.iter().for_each(|v| f(*v));
            }
        }
    }

    /// Replaces every value the operation reads.
    pub fn map_values(&mut self, mut f: impl FnMut(Value) -> Value) {
        match self {
            Op::Const(..) | Op::Slot(..) | Op::DataAddr(_) | Op::FuncAddr(_) => {}
            Op::Bin(_, a, b)
            | Op::Checked(_, _, a, b)
            | Op::Cmp(_, a, b)
            | Op::PtrAdd(a, b)
            | Op::Print(a, b)
            | Op::Store(a, b) => {
                *a = f(*a);
                *b = f(*b);
            }
            Op::Neg(a) | Op::Not(a) | Op::Cast(_, a, _) | Op::Load(_, a) | Op::Alloc(a) | Op::Free(a) => {
                *a = f(*a)
            }
            Op::MemCopy(a, b, c) => {
                *a = f(*a);
                *b = f(*b);
                *c = f(*c);
            }
            Op::Call(_, args) => args.iter_mut().for_each(|v| *v = f(*v)),
            Op::CallPtr(_, _, p, args) => {
                *p = f(*p);
                args.iter_mut().for_each(|v| *v = f(*v));
            }
        }
    }

    /// Can the operation be removed when its result is not used? (It changes nothing and cannot stop.)
    pub fn is_pure(&self) -> bool {
        match self {
            Op::Const(..)
            | Op::Cmp(..)
            | Op::Neg(_)
            | Op::Not(_)
            | Op::Cast(..)
            | Op::Slot(..)
            | Op::PtrAdd(..)
            | Op::DataAddr(_)
            | Op::FuncAddr(_)
            | Op::Load(..) => true,
            // Division can stop on zero; the rest of `Bin` cannot.
            Op::Bin(o, ..) => !matches!(o, BinOp::Div | BinOp::Rem | BinOp::UDiv | BinOp::URem),
            _ => false,
        }
    }
}

impl Term {
    pub fn for_each_value(&self, mut f: impl FnMut(Value)) {
        match self {
            Term::Jump(t) => t.args.iter().for_each(|v| f(*v)),
            Term::Branch(c, a, b) => {
                f(*c);
                a.args.iter().for_each(|v| f(*v));
                b.args.iter().for_each(|v| f(*v));
            }
            Term::Return(Some(v)) => f(*v),
            _ => {}
        }
    }

    pub fn map_values(&mut self, mut f: impl FnMut(Value) -> Value) {
        match self {
            Term::Jump(t) => t.args.iter_mut().for_each(|v| *v = f(*v)),
            Term::Branch(c, a, b) => {
                *c = f(*c);
                a.args.iter_mut().for_each(|v| *v = f(*v));
                b.args.iter_mut().for_each(|v| *v = f(*v));
            }
            Term::Return(Some(v)) => *v = f(*v),
            _ => {}
        }
    }

    pub fn targets_mut(&mut self) -> Vec<&mut Target> {
        match self {
            Term::Jump(t) => vec![t],
            Term::Branch(_, a, b) => vec![a, b],
            _ => Vec::new(),
        }
    }
}

impl Func {
    /// Replaces every use of the values in `map` (following chains) in all instructions and
    /// terminators.
    pub fn replace_uses(&mut self, map: &std::collections::HashMap<Value, Value>) {
        if map.is_empty() {
            return;
        }
        let find = |mut v: Value| {
            let mut n = 0;
            while let Some(w) = map.get(&v) {
                v = *w;
                n += 1;
                if n > map.len() {
                    break;
                }
            }
            v
        };
        for i in &mut self.insts {
            i.op.map_values(find);
        }
        for b in &mut self.blocks {
            if let Some(t) = &mut b.term {
                t.map_values(find);
            }
        }
    }

    /// How many times each value is read.
    pub fn use_counts(&self) -> Vec<u32> {
        let mut n = vec![0u32; self.values.len()];
        for b in &self.blocks {
            for &i in &b.insts {
                self.insts[i].op.for_each_value(|v| n[v.0 as usize] += 1);
            }
            if let Some(t) = &b.term {
                t.for_each_value(|v| n[v.0 as usize] += 1);
            }
        }
        n
    }

    /// The predecessors of every block (a block appears once per edge).
    pub fn predecessors(&self) -> Vec<Vec<BlockId>> {
        let mut p = vec![Vec::new(); self.blocks.len()];
        for i in 0..self.blocks.len() {
            for s in self.successors(BlockId(i as u32)) {
                p[s.0 as usize].push(BlockId(i as u32));
            }
        }
        p
    }

    /// The instruction that makes each value (`None` for parameters).
    pub fn defs(&self) -> Vec<Option<usize>> {
        let mut d = vec![None; self.values.len()];
        for b in &self.blocks {
            for &i in &b.insts {
                if let Some(r) = self.insts[i].result {
                    d[r.0 as usize] = Some(i);
                }
            }
        }
        d
    }
}
