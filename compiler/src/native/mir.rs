//! The machine IR (work item W4, design in `docs/design/mir.md`).
//!
//! A function is a list of blocks in layout order; a block is a list of instructions. The instructions
//! are the target's own (the opcode is a number the target defines), but their operands have one shape
//! for every target, so the passes in [`super::ralloc`] need to know nothing about a CPU.
//!
//! * **Registers** are virtual ([`Reg::Virt`], any number, each with a [`Class`]) or physical
//!   ([`Reg::Phys`], numbered by the target's register table). A virtual register may be written many
//!   times: a block parameter is a copy at the end of each predecessor, written as a [`PARCOPY`].
//! * **Operands** carry their role: a [`Role::Use`] is read, a [`Role::Def`] written after the
//!   instruction reads, a [`Role::UseDef`] both (the two-address form `d = d op s`), and a
//!   [`Role::EarlyDef`] is written before the instruction has read everything, so it must not share a
//!   register with a use. Rule 1 of `a11-work-items.md` is the default: a `Def` may take the register
//!   of a `Use` that dies in the same instruction, so an emitter must read every operand before it writes.
//! * **Control flow.** Branch instructions ([`IFlags::BRANCH`]) name their targets as
//!   [`Operand::Block`] and may sit anywhere in a block (a conditional branch, then the copies for the
//!   edge that falls through); what is live at the branch is what the target needs. An instruction with
//!   [`IFlags::NOFALL`] (a jump, a return, a stop) ends the block, and any other block falls into the
//!   next one in the list.
//! * **Fixed registers** are written with physical operands (`rdx:rax` for a division, `cl` for a
//!   shift, the argument registers for a call). The allocator keeps every virtual register out of a
//!   physical register while it is live; calls name the registers they destroy in `clobbers`.
//! * The allocator reserves a few registers per class ([`Target::scratch`]) for reloads and for
//!   breaking cycles of copies. Selected code never names them except through the copies the
//!   allocator makes.

use std::fmt::Write;

/// A physical register: an index into [`Target::regs`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct PReg(pub u8);

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub enum Reg {
    Virt(u32),
    Phys(PReg),
}

/// What a register holds: it decides which physical registers a virtual one can take.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Class {
    /// Integers, booleans and pointers.
    Int,
    /// A float of 32 or 64 bits.
    Float,
    /// A 128-bit vector.
    Vec,
}

impl Class {
    /// Bytes of a spill slot.
    pub fn size(self) -> u32 {
        match self {
            Class::Int | Class::Float => 8,
            Class::Vec => 16,
        }
    }
}

/// A frame object: a stack slot of the IR or a spill slot of the allocator.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct FrameId(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FrameKind {
    Slot,
    Spill,
    /// The stack arguments of the calls this function makes: at the bottom of the frame, where the
    /// callee finds them above its return address.
    Outgoing,
    /// The arguments the caller left on the stack, above the frame: the emitter gives it an
    /// address, nothing is reserved for it.
    Incoming,
}

#[derive(Clone, Copy, Debug)]
pub struct FrameObj {
    pub size: u32,
    pub align: u32,
    pub kind: FrameKind,
}

/// A set of physical registers (at most 128).
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct RegSet(pub u128);

impl RegSet {
    pub fn of(regs: &[PReg]) -> RegSet {
        let mut s = RegSet(0);
        for r in regs {
            s.insert(*r);
        }
        s
    }

    pub fn insert(&mut self, r: PReg) {
        self.0 |= 1u128 << r.0;
    }

    pub fn contains(self, r: PReg) -> bool {
        self.0 >> r.0 & 1 == 1
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn iter(self) -> impl Iterator<Item = PReg> {
        (0..128u8).filter(move |i| self.0 >> i & 1 == 1).map(PReg)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Use,
    Def,
    UseDef,
    EarlyDef,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MemBase {
    Reg(Reg),
    Frame(FrameId),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Operand {
    Reg(Reg, Role),
    Imm(i64),
    /// The address of a frame object plus a constant.
    Frame(FrameId, i32),
    /// A memory operand `[base + index * scale + disp]`; its registers are read.
    Mem {
        base: MemBase,
        index: Option<(Reg, u8)>,
        disp: i32,
    },
    /// A block of the function, the target of a branch.
    Block(u32),
    /// A name the target defines: a function, a piece of data, a stop routine.
    Sym(u32),
}

impl Operand {
    pub fn use_(r: Reg) -> Operand {
        Operand::Reg(r, Role::Use)
    }
    pub fn def(r: Reg) -> Operand {
        Operand::Reg(r, Role::Def)
    }
    pub fn use_def(r: Reg) -> Operand {
        Operand::Reg(r, Role::UseDef)
    }
    pub fn v(n: u32) -> Reg {
        Reg::Virt(n)
    }
}

/// `COPY dst, src`: `dst` is a register (a def); `src` is a register, an immediate or a frame address.
pub const COPY: u16 = 0;
/// All the copies at once, as if each source were read before any destination is written: the
/// destinations first (all defs), then as many sources. Used for the arguments of a call, the
/// parameters of a function and the block parameters of an edge.
pub const PARCOPY: u16 = 1;
/// The first opcode a target may define.
pub const FIRST_TARGET_OP: u16 = 16;

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct IFlags(pub u8);

impl IFlags {
    /// Destroys the registers in `clobbers`.
    pub const CALL: u8 = 1;
    /// Its [`Operand::Block`] operands are control-flow targets.
    pub const BRANCH: u8 = 2;
    /// Control never reaches the next instruction (a jump, a return, a stop).
    pub const NOFALL: u8 = 4;

    pub fn has(self, f: u8) -> bool {
        self.0 & f != 0
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MInst {
    pub op: u16,
    pub ops: Vec<Operand>,
    pub flags: IFlags,
    /// The physical registers the instruction destroys besides the ones it names as defs: everything
    /// a call leaves undefined, or the registers a macro uses inside itself. The allocator keeps
    /// reloads out of them and, for a call, splits the values that live across it.
    pub clobbers: RegSet,
}

impl MInst {
    pub fn new(op: u16, ops: Vec<Operand>) -> MInst {
        MInst {
            op,
            ops,
            flags: IFlags::default(),
            clobbers: RegSet::default(),
        }
    }

    pub fn copy(dst: Reg, src: Operand) -> MInst {
        MInst::new(COPY, vec![Operand::def(dst), src])
    }

    /// Several copies at once; `pairs` are `(dst, src)`.
    pub fn parcopy(pairs: Vec<(Reg, Operand)>) -> MInst {
        let mut ops: Vec<Operand> = pairs.iter().map(|(d, _)| Operand::def(*d)).collect();
        ops.extend(pairs.into_iter().map(|(_, s)| s));
        MInst::new(PARCOPY, ops)
    }

    pub fn call(mut self, clobbers: RegSet) -> MInst {
        self.flags.0 |= IFlags::CALL;
        self.clobbers = clobbers;
        self
    }

    /// Registers the instruction uses inside itself (not a call).
    pub fn clobbering(mut self, clobbers: RegSet) -> MInst {
        self.clobbers = clobbers;
        self
    }

    pub fn branch(mut self) -> MInst {
        self.flags.0 |= IFlags::BRANCH;
        self
    }

    pub fn nofall(mut self) -> MInst {
        self.flags.0 |= IFlags::NOFALL;
        self
    }
}

#[derive(Clone, Default, Debug)]
pub struct MBlock {
    pub insts: Vec<MInst>,
    /// The block a jump back leads to: the emitter may align it.
    pub loop_head: bool,
}

#[derive(Clone, Debug, Default)]
pub struct MFunc {
    pub name: String,
    pub blocks: Vec<MBlock>,
    /// The class of each virtual register.
    pub vclass: Vec<Class>,
    pub frame: Vec<FrameObj>,
    /// Filled in by the allocator: the callee-saved registers the function writes.
    pub used_callee_saved: RegSet,
    /// Texts the code refers to by number (the messages of `stop`).
    pub strings: Vec<String>,
}

impl MFunc {
    pub fn new_vreg(&mut self, c: Class) -> Reg {
        self.vclass.push(c);
        Reg::Virt(self.vclass.len() as u32 - 1)
    }

    pub fn new_frame(&mut self, size: u32, align: u32, kind: FrameKind) -> FrameId {
        self.frame.push(FrameObj { size, align, kind });
        FrameId(self.frame.len() as u32 - 1)
    }

    pub fn new_block(&mut self) -> u32 {
        self.blocks.push(MBlock::default());
        self.blocks.len() as u32 - 1
    }

    pub fn push(&mut self, b: u32, i: MInst) {
        self.blocks[b as usize].insts.push(i);
    }
}

// ---- The target ------------------------------------------------------------------------------------

pub struct RegInfo {
    pub name: &'static str,
    pub class: Class,
    pub callee_saved: bool,
}

/// What the shared passes need to know about a CPU.
pub trait Target {
    /// Every physical register, indexed by [`PReg`].
    fn regs(&self) -> &[RegInfo];
    /// The registers a virtual register of the class may live in, in the order to try them.
    fn allocatable(&self, c: Class) -> &[PReg];
    /// Registers kept out of allocation for reloads and for breaking cycles of copies: at least two
    /// per class that has allocatable registers, three for `Int` when an instruction can read three
    /// spilled values.
    fn scratch(&self, c: Class) -> &[PReg];
    fn op_name(&self, op: u16) -> &'static str;
    /// `dst = src`, registers of the class.
    fn mov(&self, c: Class, dst: PReg, src: PReg) -> MInst;
    /// `dst = imm`, an `Int` register.
    fn mov_imm(&self, dst: PReg, imm: i64) -> MInst;
    /// `dst = address of frame object + disp`.
    fn lea(&self, dst: PReg, f: FrameId, disp: i32) -> MInst;
    /// `dst = [frame object]`, a spill reload.
    fn load(&self, c: Class, dst: PReg, f: FrameId) -> MInst;
    /// `[frame object] = src`, a spill store.
    fn store(&self, c: Class, f: FrameId, src: PReg) -> MInst;
    /// `jmp block`: a branch that never falls through.
    fn jump(&self, to: u32) -> MInst;
    /// Can operand `idx` of the instruction be a memory operand (a spilled integer read from its slot
    /// instead of being reloaded first)? Only plain register uses are asked about.
    fn mem_operand(&self, _inst: &MInst, _idx: usize) -> bool {
        false
    }
}

// ---- Reading instructions --------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Acc {
    Use,
    Def,
    EarlyDef,
}

/// Calls `f` for every register the instruction touches. A `UseDef` is a use and a def.
pub fn visit_regs(i: &MInst, mut f: impl FnMut(Reg, Acc)) {
    if i.op == PARCOPY {
        let n = i.ops.len() / 2;
        for o in &i.ops[..n] {
            if let Operand::Reg(r, _) = o {
                f(*r, Acc::Def);
            }
        }
        for o in &i.ops[n..] {
            if let Operand::Reg(r, _) = o {
                f(*r, Acc::Use);
            }
        }
        return;
    }
    for o in &i.ops {
        match o {
            Operand::Reg(r, Role::Use) => f(*r, Acc::Use),
            Operand::Reg(r, Role::Def) => f(*r, Acc::Def),
            Operand::Reg(r, Role::EarlyDef) => f(*r, Acc::EarlyDef),
            Operand::Reg(r, Role::UseDef) => {
                f(*r, Acc::Use);
                f(*r, Acc::Def);
            }
            Operand::Mem { base, index, .. } => {
                if let MemBase::Reg(b) = base {
                    f(*b, Acc::Use);
                }
                if let Some((x, _)) = index {
                    f(*x, Acc::Use);
                }
            }
            _ => {}
        }
    }
}

/// The blocks control can go to after block `bi`.
pub fn successors(f: &MFunc, bi: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let b = &f.blocks[bi];
    for i in &b.insts {
        if i.flags.has(IFlags::BRANCH) {
            for o in &i.ops {
                if let Operand::Block(t) = o {
                    if !out.contains(&(*t as usize)) {
                        out.push(*t as usize);
                    }
                }
            }
        }
    }
    let falls = b.insts.last().is_none_or(|i| !i.flags.has(IFlags::NOFALL));
    if falls && bi + 1 < f.blocks.len() && !out.contains(&(bi + 1)) {
        out.push(bi + 1);
    }
    out
}

/// A set of small numbers.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Bits(Vec<u64>);

impl Bits {
    pub fn new(n: usize) -> Bits {
        Bits(vec![0; n.div_ceil(64)])
    }
    pub fn insert(&mut self, i: usize) {
        self.0[i / 64] |= 1 << (i % 64);
    }
    pub fn remove(&mut self, i: usize) {
        self.0[i / 64] &= !(1 << (i % 64));
    }
    pub fn contains(&self, i: usize) -> bool {
        self.0[i / 64] >> (i % 64) & 1 == 1
    }
    /// Adds everything in `o`; true when something was new.
    pub fn union(&mut self, o: &Bits) -> bool {
        let mut changed = false;
        for (a, b) in self.0.iter_mut().zip(&o.0) {
            let n = *a | *b;
            changed |= n != *a;
            *a = n;
        }
        changed
    }
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.0.iter().enumerate().flat_map(|(w, &bits)| {
            (0..64)
                .filter(move |b| bits >> b & 1 == 1)
                .map(move |b| w * 64 + b)
        })
    }
}

/// The index of a register in the combined numbering: physical registers first, then virtual ones.
pub fn reg_id(r: Reg, nphys: usize) -> usize {
    match r {
        Reg::Phys(p) => p.0 as usize,
        Reg::Virt(v) => nphys + v as usize,
    }
}

pub struct Liveness {
    /// Registers live at the start of each block.
    pub live_in: Vec<Bits>,
    /// Registers live at the end of each block: what the next block needs, when control falls into it.
    pub live_end: Vec<Bits>,
}

/// The registers live before instruction `i` given those live after it (`now`); `live_in` holds the
/// sets of the branch targets.
pub fn step_back(i: &MInst, now: &mut Bits, live_in: &[Bits], nphys: usize) {
    if i.flags.has(IFlags::NOFALL) && !i.flags.has(IFlags::BRANCH) {
        // A return or a stop: nothing after it.
        for w in now.0.iter_mut() {
            *w = 0;
        }
    }
    if i.flags.has(IFlags::BRANCH) {
        if i.flags.has(IFlags::NOFALL) {
            for w in now.0.iter_mut() {
                *w = 0;
            }
        }
        for o in &i.ops {
            if let Operand::Block(t) = o {
                now.union(&live_in[*t as usize]);
            }
        }
    }
    let (mut uses, mut defs) = (Vec::new(), Vec::new());
    visit_regs(i, |r, a| match a {
        Acc::Use => uses.push(reg_id(r, nphys)),
        _ => defs.push(reg_id(r, nphys)),
    });
    for d in defs {
        now.remove(d);
    }
    for u in uses {
        now.insert(u);
    }
}

/// Registers live at the start and the end of each block (ids as in [`reg_id`]).
pub fn liveness(f: &MFunc, nphys: usize) -> Liveness {
    let n = nphys + f.vclass.len();
    let nb = f.blocks.len();
    let mut live_in: Vec<Bits> = (0..nb).map(|_| Bits::new(n)).collect();
    let mut live_end: Vec<Bits> = (0..nb).map(|_| Bits::new(n)).collect();
    loop {
        let mut changed = false;
        for bi in (0..nb).rev() {
            let b = &f.blocks[bi];
            let falls = b.insts.last().is_none_or(|i| !i.flags.has(IFlags::NOFALL));
            let mut now = Bits::new(n);
            if falls && bi + 1 < nb {
                now.union(&live_in[bi + 1]);
            }
            live_end[bi] = now.clone();
            for i in b.insts.iter().rev() {
                step_back(i, &mut now, &live_in, nphys);
            }
            if now != live_in[bi] {
                live_in[bi] = now;
                changed = true;
            }
        }
        if !changed {
            return Liveness { live_in, live_end };
        }
    }
}

// ---- Checking --------------------------------------------------------------------------------------

/// Checks the rules in the module docs; the error names the function, block and instruction.
pub fn verify(f: &MFunc, t: &dyn Target) -> Result<(), String> {
    let nphys = t.regs().len();
    let nb = f.blocks.len();
    let err = |bi: usize, k: usize, m: &str| -> Result<(), String> {
        Err(format!("mir `{}` b{bi}:{k}: {m}", f.name))
    };
    if nb == 0 {
        return Err(format!("mir `{}`: no blocks", f.name));
    }
    let class_of = |r: Reg| match r {
        Reg::Virt(v) => f.vclass.get(v as usize).copied(),
        Reg::Phys(p) => t.regs().get(p.0 as usize).map(|i| i.class),
    };
    for (bi, b) in f.blocks.iter().enumerate() {
        for (k, i) in b.insts.iter().enumerate() {
            if i.op >= 2 && i.op < FIRST_TARGET_OP {
                return err(bi, k, "an opcode below 16 that is not COPY or PARCOPY");
            }
            let is_branch = i.flags.has(IFlags::BRANCH);
            if i.flags.has(IFlags::NOFALL) && k + 1 != b.insts.len() {
                return err(bi, k, "an instruction after one that never falls through");
            }
            let mut bad = None;
            visit_regs(i, |r, _| match r {
                Reg::Virt(v) if v as usize >= f.vclass.len() => {
                    bad = Some("a virtual register out of range")
                }
                Reg::Phys(p) if p.0 as usize >= nphys => {
                    bad = Some("a physical register out of range")
                }
                _ => {}
            });
            if let Some(m) = bad {
                return err(bi, k, m);
            }
            for o in &i.ops {
                match o {
                    Operand::Block(t) if *t as usize >= nb => {
                        return err(bi, k, "a block out of range")
                    }
                    Operand::Block(_) if !is_branch => {
                        return err(bi, k, "a block operand without the branch flag")
                    }
                    Operand::Frame(id, _)
                    | Operand::Mem {
                        base: MemBase::Frame(id),
                        ..
                    } if id.0 as usize >= f.frame.len() => {
                        return err(bi, k, "a frame object out of range")
                    }
                    _ => {}
                }
            }
            match i.op {
                COPY => {
                    let ok = matches!(i.ops.as_slice(), [Operand::Reg(_, Role::Def), s] if
                        matches!(s, Operand::Reg(_, Role::Use) | Operand::Imm(_) | Operand::Frame(..)));
                    if !ok {
                        return err(bi, k, "COPY wants `dst(def), src(use|imm|frame)`");
                    }
                    if let [Operand::Reg(d, _), Operand::Reg(s, _)] = i.ops.as_slice() {
                        if class_of(*d) != class_of(*s) {
                            return err(bi, k, "COPY between classes");
                        }
                    }
                }
                PARCOPY => {
                    let n = i.ops.len() / 2;
                    if i.ops.len() % 2 != 0 {
                        return err(bi, k, "PARCOPY with an odd number of operands");
                    }
                    let mut seen = Vec::new();
                    for (d, s) in i.ops[..n].iter().zip(&i.ops[n..]) {
                        let Operand::Reg(d, Role::Def) = d else {
                            return err(bi, k, "PARCOPY destination is not a def register");
                        };
                        if seen.contains(d) {
                            return err(bi, k, "PARCOPY writes a register twice");
                        }
                        seen.push(*d);
                        match s {
                            Operand::Reg(s, Role::Use) if class_of(*s) != class_of(*d) => {
                                return err(bi, k, "PARCOPY between classes")
                            }
                            Operand::Reg(_, Role::Use) | Operand::Imm(_) | Operand::Frame(..) => {}
                            _ => {
                                return err(
                                    bi,
                                    k,
                                    "PARCOPY source is not a use, immediate or frame address",
                                )
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        let last = b.insts.last();
        if bi + 1 == nb && last.is_none_or(|i| !i.flags.has(IFlags::NOFALL)) {
            return err(
                bi,
                b.insts.len(),
                "the last block falls off the end of the function",
            );
        }
    }
    // Nothing but physical registers (the parameters) may be live at the entry.
    let live = liveness(f, nphys);
    if let Some(v) = live.live_in[0].iter().find(|&r| r >= nphys) {
        return Err(format!(
            "mir `{}`: v{} is read on a path where it was never written",
            f.name,
            v - nphys
        ));
    }
    Ok(())
}

// ---- Printing --------------------------------------------------------------------------------------

fn reg_name(r: Reg, f: &MFunc, t: &dyn Target) -> String {
    match r {
        Reg::Phys(p) => t.regs()[p.0 as usize].name.to_string(),
        Reg::Virt(v) => {
            let c = match f.vclass.get(v as usize) {
                Some(Class::Int) | None => "",
                Some(Class::Float) => "f",
                Some(Class::Vec) => "x",
            };
            format!("%{c}{v}")
        }
    }
}

impl MFunc {
    /// The text form used by tests and by `pelago mir`.
    pub fn print(&self, t: &dyn Target) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "mfunc {}:", self.name);
        for (i, o) in self.frame.iter().enumerate() {
            let _ = writeln!(
                s,
                "  frame{i}: {} bytes align {} {:?}",
                o.size, o.align, o.kind
            );
        }
        for (bi, b) in self.blocks.iter().enumerate() {
            let _ = writeln!(s, "b{bi}:");
            for i in &b.insts {
                let name = match i.op {
                    COPY => "copy",
                    PARCOPY => "parcopy",
                    op => t.op_name(op),
                };
                let ops: Vec<String> = i
                    .ops
                    .iter()
                    .map(|o| match o {
                        Operand::Reg(r, role) => {
                            let n = reg_name(*r, self, t);
                            match role {
                                Role::Use | Role::Def => n,
                                Role::UseDef => format!("{n}!"),
                                Role::EarlyDef => format!("{n}*"),
                            }
                        }
                        Operand::Imm(n) => format!("#{n}"),
                        Operand::Frame(id, d) => format!(
                            "&frame{}{}",
                            id.0,
                            if *d != 0 {
                                format!("{d:+}")
                            } else {
                                String::new()
                            }
                        ),
                        Operand::Mem { base, index, disp } => {
                            let b = match base {
                                MemBase::Reg(r) => reg_name(*r, self, t),
                                MemBase::Frame(id) => format!("frame{}", id.0),
                            };
                            let x = index.map_or(String::new(), |(r, sc)| {
                                format!("+{}*{sc}", reg_name(r, self, t))
                            });
                            let d = if *disp != 0 {
                                format!("{disp:+}")
                            } else {
                                String::new()
                            };
                            format!("[{b}{x}{d}]")
                        }
                        Operand::Block(b) => format!("b{b}"),
                        Operand::Sym(n) => format!("@{n}"),
                    })
                    .collect();
                let _ = write!(s, "  {name} {}", ops.join(", "));
                if i.flags.has(IFlags::CALL) {
                    let c: Vec<&str> = i
                        .clobbers
                        .iter()
                        .map(|r| t.regs()[r.0 as usize].name)
                        .collect();
                    let _ = write!(s, "  ; clobbers {}", c.join(" "));
                }
                s.push('\n');
            }
        }
        s
    }
}
