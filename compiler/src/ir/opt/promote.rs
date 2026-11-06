//! Promote slots to values (mem2reg, after Braun et al., "Simple and Efficient Construction of SSA
//! Form"). A slot whose address is only ever the address of a `Load` or `Store` of one type becomes
//! an SSA value; block parameters carry it across control flow. A read before any write gives zero,
//! which is what a fresh slot holds in the interpreter.

use super::fx::{HashMap, HashSet};
use crate::ir::*;

pub fn run(f: &mut Func) -> bool {
    // A jump back to the entry block would need a parameter for the entry; leave such functions.
    if f.blocks.iter().any(|b| {
        b.term.as_ref().is_some_and(|t| {
            let mut hit = false;
            let mut t = t.clone();
            for tg in t.targets_mut() {
                hit |= tg.block == BlockId(0);
            }
            hit
        })
    }) {
        return false;
    }
    let slots = promotable(f);
    if slots.is_empty() {
        return false;
    }
    Promoter::new(f, slots).run();
    true
}

/// The slots that can live in registers, with the type of their contents.
fn promotable(f: &Func) -> HashMap<Value, IrTy> {
    let mut slots: HashMap<Value, (u32, Option<IrTy>)> = HashMap::default();
    for b in &f.blocks {
        for &i in &b.insts {
            if let (Op::Slot(size, _), Some(r)) = (&f.insts[i].op, f.insts[i].result) {
                slots.insert(r, (*size, None));
            }
        }
    }
    let mut bad: HashSet<Value> = HashSet::default();
    fn note(
        s: Value,
        ty: IrTy,
        slots: &mut HashMap<Value, (u32, Option<IrTy>)>,
        bad: &mut HashSet<Value>,
    ) {
        if let Some((size, t)) = slots.get_mut(&s) {
            match t {
                None if ty.size() == *size && ty != IrTy::Void => *t = Some(ty),
                Some(old) if *old == ty => {}
                _ => {
                    bad.insert(s);
                }
            }
        }
    }
    for b in &f.blocks {
        for &i in &b.insts {
            match &f.insts[i].op {
                Op::Load(ty, a) => note(*a, *ty, &mut slots, &mut bad),
                Op::Store(a, v) => {
                    note(*a, f.ty(*v), &mut slots, &mut bad);
                    // The slot's own address stored somewhere escapes.
                    if slots.contains_key(v) {
                        bad.insert(*v);
                    }
                }
                op => op.for_each_value(|v| {
                    if slots.contains_key(&v) {
                        bad.insert(v);
                    }
                }),
            }
        }
        if let Some(t) = &b.term {
            t.for_each_value(|v| {
                if slots.contains_key(&v) {
                    bad.insert(v);
                }
            });
        }
    }
    slots
        .into_iter()
        .filter_map(|(s, (_, t))| t.filter(|_| !bad.contains(&s)).map(|t| (s, t)))
        .collect()
}

struct Promoter<'f> {
    f: &'f mut Func,
    slots: HashMap<Value, IrTy>,
    preds: Vec<Vec<BlockId>>,
    /// The last value stored to each slot in each block (before replacement).
    last: HashMap<(Value, usize), Value>,
    /// The value of a slot on entry to a block.
    entry: HashMap<(Value, usize), Value>,
    /// What replaces a load result or a trivial parameter.
    replace: HashMap<Value, Value>,
    /// The parameters added to blocks: (block, parameter, slot, arguments per incoming edge).
    added: Vec<Added>,
    zero: HashMap<IrTy, Value>,
}

struct Added {
    block: usize,
    param: Value,
    args: Vec<(BlockId, Value)>,
}

impl<'f> Promoter<'f> {
    fn new(f: &'f mut Func, slots: HashMap<Value, IrTy>) -> Self {
        let preds = f.predecessors();
        Promoter {
            f,
            slots,
            preds,
            last: HashMap::default(),
            entry: HashMap::default(),
            replace: HashMap::default(),
            added: Vec::new(),
            zero: HashMap::default(),
        }
    }

    fn run(mut self) {
        // The last store in each block.
        for bi in 0..self.f.blocks.len() {
            for &i in &self.f.blocks[bi].insts.clone() {
                if let Op::Store(a, v) = self.f.insts[i].op {
                    if self.slots.contains_key(&a) {
                        self.last.insert((a, bi), v);
                    }
                }
            }
        }
        // Loads become the value in force at that point.
        let mut dead: HashSet<usize> = HashSet::default();
        for bi in 0..self.f.blocks.len() {
            let mut cur: HashMap<Value, Value> = HashMap::default();
            for &i in &self.f.blocks[bi].insts.clone() {
                let (op, result) = (self.f.insts[i].op.clone(), self.f.insts[i].result);
                match op {
                    Op::Load(_, a) if self.slots.contains_key(&a) => {
                        let v = match cur.get(&a) {
                            Some(v) => *v,
                            None => self.read_entry(a, bi),
                        };
                        if let Some(r) = result {
                            self.replace.insert(r, v);
                        }
                        dead.insert(i);
                    }
                    Op::Store(a, v) if self.slots.contains_key(&a) => {
                        cur.insert(a, v);
                        dead.insert(i);
                    }
                    Op::Slot(..) if result.is_some_and(|r| self.slots.contains_key(&r)) => {
                        dead.insert(i);
                    }
                    _ => {}
                }
            }
        }
        // Add the parameters and the arguments that feed them.
        let added = std::mem::take(&mut self.added);
        for a in &added {
            self.f.blocks[a.block].params.push(a.param);
        }
        for a in &added {
            for (pred, v) in &a.args {
                if let Some(t) = &mut self.f.blocks[pred.0 as usize].term {
                    for tg in t.targets_mut() {
                        if tg.block.0 as usize == a.block {
                            tg.args.push(*v);
                        }
                    }
                }
            }
        }
        for b in &mut self.f.blocks {
            b.insts.retain(|i| !dead.contains(i));
        }
        self.f.replace_uses(&self.replace);
        self.remove_trivial();
    }

    fn zero_of(&mut self, ty: IrTy) -> Value {
        if let Some(v) = self.zero.get(&ty) {
            return *v;
        }
        let sp = crate::span::Span::new(0, 0);
        let mut made: Vec<usize> = Vec::new();
        let v = if ty == IrTy::Ptr {
            // A null pointer is an integer zero turned into a pointer.
            let c = self.f.new_value(IrTy::I64);
            self.f.insts.push(Inst {
                result: Some(c),
                op: Op::Const(IrTy::I64, Const::Int(0)),
                span: sp,
            });
            made.push(self.f.insts.len() - 1);
            let p = self.f.new_value(IrTy::Ptr);
            self.f.insts.push(Inst {
                result: Some(p),
                op: Op::Cast(CastKind::IntToPtr, c, IrTy::Ptr),
                span: sp,
            });
            made.push(self.f.insts.len() - 1);
            p
        } else {
            let c = match ty {
                IrTy::Bool => Const::Bool(false),
                IrTy::F32 | IrTy::F64 => Const::Float(0.0),
                _ => Const::Int(0),
            };
            let v = self.f.new_value(ty);
            self.f.insts.push(Inst {
                result: Some(v),
                op: Op::Const(ty, c),
                span: sp,
            });
            made.push(self.f.insts.len() - 1);
            v
        };
        for (k, id) in made.into_iter().enumerate() {
            self.f.blocks[0].insts.insert(k, id);
        }
        self.zero.insert(ty, v);
        v
    }

    /// The value of `slot` when control enters block `b`.
    fn read_entry(&mut self, slot: Value, start: usize) -> Value {
        if let Some(v) = self.entry.get(&(slot, start)) {
            return *v;
        }
        let ty = self.slots[&slot];
        // Walk up through blocks with one predecessor and no store of their own.
        let mut b = start;
        let mut seen: HashSet<usize> = HashSet::default();
        let v = loop {
            if let Some(v) = self.entry.get(&(slot, b)) {
                break *v;
            }
            let preds = self.preds[b].clone();
            if b == 0 || preds.is_empty() || !seen.insert(b) {
                break self.zero_of(ty);
            }
            if preds.len() == 1 {
                let p = preds[0].0 as usize;
                if let Some(v) = self.last.get(&(slot, p)) {
                    break *v;
                }
                b = p;
                continue;
            }
            // Several predecessors: a parameter, made before its arguments so loops find it.
            let p = self.f.new_value(ty);
            self.entry.insert((slot, b), p);
            let mut args = Vec::new();
            let mut done: HashSet<BlockId> = HashSet::default();
            for pred in &preds {
                if !done.insert(*pred) {
                    continue;
                }
                let v = self.read_exit(slot, pred.0 as usize);
                args.push((*pred, v));
            }
            self.added.push(Added {
                block: b,
                param: p,
                args,
            });
            break p;
        };
        self.entry.insert((slot, start), v);
        v
    }

    /// The value of `slot` when control leaves block `b`.
    fn read_exit(&mut self, slot: Value, b: usize) -> Value {
        match self.last.get(&(slot, b)) {
            Some(v) => *v,
            None => self.read_entry(slot, b),
        }
    }

    /// A parameter whose arguments are all one value (or itself) is that value.
    fn remove_trivial(&mut self) {
        loop {
            let mut map: HashMap<Value, Value> = HashMap::default();
            let mut changed: Vec<(usize, usize)> = Vec::new();
            for bi in 1..self.f.blocks.len() {
                let params = self.f.blocks[bi].params.clone();
                for (pi, p) in params.iter().enumerate() {
                    let mut same: Option<Value> = None;
                    let mut trivial = true;
                    for pred in &self.preds[bi] {
                        let Some(t) = &self.f.blocks[pred.0 as usize].term else {
                            continue;
                        };
                        let mut t = t.clone();
                        for tg in t.targets_mut() {
                            if tg.block.0 as usize != bi {
                                continue;
                            }
                            let Some(&a) = tg.args.get(pi) else {
                                trivial = false;
                                continue;
                            };
                            if a == *p {
                                continue;
                            }
                            match same {
                                None => same = Some(a),
                                Some(s) if s == a => {}
                                Some(_) => trivial = false,
                            }
                        }
                    }
                    if trivial {
                        if let Some(s) = same {
                            map.insert(*p, s);
                            changed.push((bi, pi));
                        }
                    }
                }
            }
            if map.is_empty() {
                return;
            }
            // Remove the parameters (highest index first per block) and their arguments.
            changed.sort_by(|a, b| b.cmp(a));
            for (bi, pi) in &changed {
                self.f.blocks[*bi].params.remove(*pi);
                for b in &mut self.f.blocks {
                    if let Some(t) = &mut b.term {
                        for tg in t.targets_mut() {
                            if tg.block.0 as usize == *bi && pi < &tg.args.len() {
                                tg.args.remove(*pi);
                            }
                        }
                    }
                }
            }
            self.f.replace_uses(&map);
            // The replaced values may themselves have been parameters just removed: `replace_uses`
            // follows chains, so one more round settles the rest.
        }
    }
}
