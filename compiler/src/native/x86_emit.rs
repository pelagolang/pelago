//! From allocated x86-64 MIR to bytes (design in `docs/design/mir.md`).
//!
//! The frame has no frame pointer. After the pushes of the callee-saved registers and one `sub rsp`
//! the stack pointer stays where it is for the whole function, so every frame object is `[rsp + k]`
//! and the arguments the caller left on the stack are at `[rsp + frame + 8 * saved + 8 + 8 * i]`.
//! The size of the `sub` keeps `rsp` 16-byte aligned for calls. A call that has stack arguments
//! writes them into a frame object of its own (the selector makes it), so nothing is ever pushed
//! except in the prologue and by the macros that save `rdi` around a string instruction.

use super::elf::Blob;
use super::mir::*;
use super::rt::Runtime;
use super::x86::{self, Alu, Asm, Cc, Fp, Label, Sect, Shift, Sse, VAlu};
use super::x86_mir::{cc_of, op, SYM_DIV0, SYM_FUNC, SYM_OVERFLOW, SYM_SHIFT};

/// What the emitter takes from the generator.
pub struct Env<'a> {
    pub a: &'a mut Asm,
    pub ro: &'a mut Blob,
    pub funcs: &'a [Label],
    pub data_off: &'a [u32],
    /// The shared stop routines, by `SYM_*` number.
    pub stops: [Label; 4],
    pub rt: &'a Runtime,
}

fn gpr(p: PReg) -> x86::Reg {
    debug_assert!(p.0 < 16, "{p:?} is not a general register");
    x86::Reg(p.0)
}

fn xr(p: PReg) -> x86::Xmm {
    debug_assert!(p.0 >= 16, "{p:?} is not an SSE register");
    x86::Xmm(p.0 - 16)
}

fn is_xmm(p: PReg) -> bool {
    p.0 >= 16
}

fn phys(o: &Operand) -> PReg {
    match o {
        Operand::Reg(Reg::Phys(p), _) => *p,
        o => panic!("an unallocated or missing register: {o:?}"),
    }
}

fn imm(o: &Operand) -> i64 {
    match o {
        Operand::Imm(n) => *n,
        o => panic!("an immediate was expected: {o:?}"),
    }
}

struct Frame {
    /// Offset of each object from `rsp` (the incoming arguments from `rsp` too).
    off: Vec<i32>,
    saves: Vec<PReg>,
    /// The `sub rsp` amount.
    size: i32,
}

fn layout(f: &MFunc) -> Frame {
    let saves: Vec<PReg> = f.used_callee_saved.iter().collect();
    let mut off = vec![0i32; f.frame.len()];
    let mut at = 0i32;
    // The outgoing arguments first: they sit at `[rsp]`.
    let order = (0..f.frame.len())
        .filter(|&i| f.frame[i].kind == FrameKind::Outgoing)
        .chain((0..f.frame.len()).filter(|&i| f.frame[i].kind != FrameKind::Outgoing));
    for i in order {
        let o = &f.frame[i];
        if o.kind == FrameKind::Incoming {
            continue;
        }
        let al = (o.align as i32).max(1);
        at = (at + al - 1) / al * al;
        off[i] = at;
        at += o.size as i32;
    }
    // Entry has `rsp` 8 off a 16-byte boundary and each push moves it by 8. A function that calls
    // (or keeps an object that wants 16 bytes) puts it back on one; a leaf does not bother.
    let base = 8 + 8 * saves.len() as i32;
    let aligned = f
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .any(|i| i.flags.has(IFlags::CALL))
        || f.frame.iter().any(|o| o.align >= 16);
    let mut size = (at + 7) / 8 * 8;
    while aligned && (base + size) % 16 != 0 {
        size += 8;
    }
    for (i, o) in f.frame.iter().enumerate() {
        if o.kind == FrameKind::Incoming {
            off[i] = size + base;
        }
    }
    Frame { off, saves, size }
}

/// The out-of-line half of a `CheckedIf`: after the overflow jump, stop when the test holds, else go
/// back to `resume`. Registers are as they were at the jump.
struct Cold {
    label: Label,
    resume: Label,
    stop: Label,
    test: Vec<Operand>,
}

struct Em<'e, 'a> {
    env: &'e mut Env<'a>,
    fr: Frame,
    labels: Vec<Label>,
    strings: &'e [String],
    cold: Vec<Cold>,
}

impl Em<'_, '_> {
    fn mem(&self, o: &Operand) -> x86::Mem {
        match o {
            Operand::Mem { base, index, disp } => {
                let (b, extra) = match base {
                    MemBase::Reg(r) => (
                        gpr(match r {
                            Reg::Phys(p) => *p,
                            r => panic!("an unallocated register {r:?}"),
                        }),
                        0,
                    ),
                    MemBase::Frame(id) => (x86::RSP, self.fr.off[id.0 as usize]),
                };
                let index = index.map(|(r, s)| {
                    (
                        gpr(match r {
                            Reg::Phys(p) => p,
                            r => panic!("an unallocated register {r:?}"),
                        }),
                        s,
                    )
                });
                x86::Mem {
                    base: b,
                    index,
                    disp: disp + extra,
                }
            }
            o => panic!("a memory operand was expected: {o:?}"),
        }
    }

    fn frame_mem(&self, id: FrameId, disp: i32) -> x86::Mem {
        x86::mem(x86::RSP, self.fr.off[id.0 as usize] + disp)
    }

    fn block_label(&self, o: &Operand) -> Label {
        match o {
            Operand::Block(b) => self.labels[*b as usize],
            o => panic!("a block was expected: {o:?}"),
        }
    }

    fn stop_label(&self, o: &Operand) -> Label {
        match o {
            Operand::Sym(n) => self.env.stops[*n as usize],
            o => panic!("a stop routine was expected: {o:?}"),
        }
    }

    fn prologue(&mut self) {
        let a = &mut *self.env.a;
        for r in &self.fr.saves {
            a.push(gpr(*r));
        }
        let size = self.fr.size;
        if size > 0 {
            // A big frame must not step over the guard: touch each page, top down, first.
            let pages = size / 4096;
            if pages <= 8 {
                for k in 1..=pages {
                    a.alu_mi(Alu::Or, x86::mem(x86::RSP, -4096 * k), 0, 8);
                }
            } else {
                // A loop, not a probe per page in the code: a 40 MB array is 10 000 of them.
                let top = a.new_label();
                a.lea(x86::R11, x86::mem(x86::RSP, -4096 * pages));
                a.mov_rr(x86::R10, x86::RSP, 8);
                a.bind(top);
                a.alu_ri(Alu::Sub, x86::R10, 4096, 8);
                a.alu_mi(Alu::Or, x86::mem(x86::R10, 0), 0, 8);
                a.alu_rr(Alu::Cmp, x86::R10, x86::R11, 8);
                a.jcc(Cc::A, top);
            }
            a.alu_ri(Alu::Sub, x86::RSP, size, 8);
        }
    }

    fn epilogue(&mut self) {
        let a = &mut *self.env.a;
        if self.fr.size > 0 {
            a.alu_ri(Alu::Add, x86::RSP, self.fr.size, 8);
        }
        for r in self.fr.saves.iter().rev() {
            a.pop(gpr(*r));
        }
        a.ret();
    }

    /// `dst = src` for registers, immediates and frame addresses.
    fn copy(&mut self, d: PReg, s: &Operand) {
        match s {
            Operand::Reg(Reg::Phys(s), _) => {
                if d == *s {
                    return;
                }
                if is_xmm(d) {
                    self.env.a.fmov_rr(xr(d), xr(*s));
                } else {
                    self.env.a.mov_rr(gpr(d), gpr(*s), 8);
                }
            }
            Operand::Imm(n) => self.env.a.mov_ri(gpr(d), *n),
            Operand::Frame(id, disp) => {
                let m = self.frame_mem(*id, *disp);
                self.env.a.lea(gpr(d), m);
            }
            o => panic!("copy from {o:?}"),
        }
    }

    fn alu(&mut self, alu: Alu, ops: &[Operand]) {
        let d = gpr(phys(&ops[0]));
        match &ops[1] {
            Operand::Reg(..) => self.env.a.alu_rr(alu, d, gpr(phys(&ops[1])), 8),
            Operand::Imm(n) => self.env.a.alu_ri(alu, d, *n as i32, 8),
            m @ Operand::Mem { .. } => {
                let m = self.mem(m);
                self.env.a.alu_rm(alu, d, m, 8);
            }
            o => panic!("alu with {o:?}"),
        }
    }

    /// `cmp a, b` where `b` is a register or an immediate.
    fn cmp(&mut self, a: &Operand, b: &Operand) {
        let ra = gpr(phys(a));
        match b {
            Operand::Imm(n) => self.env.a.alu_ri(Alu::Cmp, ra, *n as i32, 8),
            m @ Operand::Mem { .. } => {
                let m = self.mem(m);
                self.env.a.alu_rm(Alu::Cmp, ra, m, 8);
            }
            b => self.env.a.alu_rr(Alu::Cmp, ra, gpr(phys(b)), 8),
        }
    }

    fn mask(&mut self, r: x86::Reg, bits: i64) {
        match bits {
            8 => self.env.a.extend_rr(r, r, 1, false, false),
            16 => self.env.a.extend_rr(r, r, 2, false, false),
            32 => self.env.a.mov_rr(r, r, 4),
            _ => {}
        }
    }

    fn copy_bytes(&mut self, to: x86::Mem, from: x86::Mem, len: i32) {
        let at = |m: x86::Mem, k: i32| x86::Mem {
            disp: m.disp + k,
            ..m
        };
        let mut k = 0;
        for w in [8, 4, 2, 1] {
            while len - k >= w {
                self.env.a.load(x86::RAX, at(from, k), w as u8);
                self.env.a.store(at(to, k), x86::RAX, w as u8);
                k += w;
            }
        }
    }

    fn zero(&mut self, id: FrameId, disp: i32, size: i32) {
        let off = self.fr.off[id.0 as usize] + disp;
        let a = &mut *self.env.a;
        if size <= 128 {
            let mut k = 0;
            if size >= 16 {
                a.fxor(x86::Xmm(15), x86::Xmm(15));
                while size - k >= 16 {
                    a.movups_store(x86::mem(x86::RSP, off + k), x86::Xmm(15));
                    k += 16;
                }
            }
            for w in [8, 4, 2, 1] {
                while size - k >= w {
                    a.store_imm(x86::mem(x86::RSP, off + k), 0, w as u8);
                    k += w;
                }
            }
        } else {
            // `rdi` may hold a value: the string instruction needs it.
            a.push(x86::RDI);
            a.lea(x86::RDI, x86::mem(x86::RSP, off + 8));
            a.alu_rr(Alu::Xor, x86::RAX, x86::RAX, 4);
            a.mov_ri(x86::RCX, size as i64);
            a.rep_stosb();
            a.pop(x86::RDI);
        }
    }

    /// The jump taken on overflow. An instruction that carries a test (`tail`, from a `CheckedIf`)
    /// jumps to a cold stub that stops only when the test holds.
    fn ov_jump(&mut self, cc: Cc, stop: Label, tail: &[Operand]) {
        if tail.is_empty() {
            self.env.a.jcc(cc, stop);
            return;
        }
        let (label, resume) = (self.env.a.new_label(), self.env.a.new_label());
        self.env.a.jcc(cc, label);
        self.env.a.bind(resume);
        self.cold.push(Cold {
            label,
            resume,
            stop,
            test: tail.to_vec(),
        });
    }

    fn cold_stubs(&mut self) {
        for c in std::mem::take(&mut self.cold) {
            self.env.a.bind(c.label);
            match imm(&c.test[0]) {
                // `[0, cond, when]`: stop when the bool equals `when`.
                0 => {
                    let r = gpr(phys(&c.test[1]));
                    self.env.a.test_rr(r, r, 4);
                    let cc = if imm(&c.test[2]) == 1 { Cc::E } else { Cc::Ne };
                    self.env.a.jcc(cc, c.resume);
                }
                // `[1, cc, a, b]`: stop when the comparison holds.
                _ => {
                    self.cmp(&c.test[2], &c.test[3]);
                    self.env.a.jcc(cc_of(imm(&c.test[1])).invert(), c.resume);
                }
            }
            self.env.a.jmp(c.stop);
        }
    }

    fn fmem_op(&self, o: &Operand) -> Fp {
        Fp::Reg(xr(phys(o)))
    }

    fn inst(&mut self, i: &MInst) {
        let ops = &i.ops;
        match i.op {
            COPY => {
                let d = phys(&ops[0]);
                self.copy(d, &ops[1]);
            }
            op::ADD => self.alu(Alu::Add, ops),
            op::SUB => self.alu(Alu::Sub, ops),
            op::AND => self.alu(Alu::And, ops),
            op::OR => self.alu(Alu::Or, ops),
            op::XOR => self.alu(Alu::Xor, ops),
            op::IMUL => self
                .env
                .a
                .imul_rr(gpr(phys(&ops[0])), gpr(phys(&ops[1])), 8),
            op::IMULI => self.env.a.imul_rri(
                gpr(phys(&ops[0])),
                gpr(phys(&ops[1])),
                imm(&ops[2]) as i32,
                8,
            ),
            op::LEA => {
                let m = self.mem(&ops[1]);
                self.env.a.lea(gpr(phys(&ops[0])), m);
            }
            op::NEG => self.env.a.neg(gpr(phys(&ops[0])), 8),
            op::NOT => self.env.a.not(gpr(phys(&ops[0])), 8),
            op::SHL_I => {
                self.env
                    .a
                    .shift_imm(Shift::Shl, gpr(phys(&ops[0])), imm(&ops[1]) as u8, 8)
            }
            op::SHR_I => {
                self.env
                    .a
                    .shift_imm(Shift::Shr, gpr(phys(&ops[0])), imm(&ops[1]) as u8, 8)
            }
            op::SAR_I => {
                self.env
                    .a
                    .shift_imm(Shift::Sar, gpr(phys(&ops[0])), imm(&ops[1]) as u8, 8)
            }
            op::MASK => self.mask(gpr(phys(&ops[0])), imm(&ops[1])),
            op::EXT => self.env.a.extend_rr(
                gpr(phys(&ops[0])),
                gpr(phys(&ops[1])),
                imm(&ops[2]) as u8,
                imm(&ops[3]) == 1,
                true,
            ),
            op::LOAD => {
                let m = self.mem(&ops[1]);
                self.env
                    .a
                    .load_ext(gpr(phys(&ops[0])), m, imm(&ops[2]) as u8, false, true);
            }
            op::STORE => {
                let m = self.mem(&ops[0]);
                let size = imm(&ops[2]) as u8;
                match &ops[1] {
                    Operand::Imm(n) => self.env.a.store_imm(m, *n as i32, size),
                    s => self.env.a.store(m, gpr(phys(s)), size),
                }
            }
            op::CMPJCC => {
                let start = self.env.a.here();
                self.cmp(&ops[1], &ops[2]);
                // The compare and the jump after it run as one micro-operation: keep them in one 32-byte block.
                self.env.a.guard_jump(start, 6);
                let l = self.block_label(&ops[3]);
                self.env.a.jcc(cc_of(imm(&ops[0])), l);
            }
            op::TESTJCC => {
                let start = self.env.a.here();
                let r = gpr(phys(&ops[1]));
                self.env.a.test_rr(r, r, 1);
                self.env.a.guard_jump(start, 6);
                let l = self.block_label(&ops[2]);
                self.env.a.jcc(cc_of(imm(&ops[0])), l);
            }
            op::JMP => {
                let l = self.block_label(&ops[0]);
                self.env.a.jmp(l);
            }
            op::CMPSET => {
                let d = gpr(phys(&ops[0]));
                self.cmp(&ops[2], &ops[3]);
                self.env.a.setcc(cc_of(imm(&ops[1])), d);
                self.env.a.extend_rr(d, d, 1, false, false);
            }
            op::CMPCMOV => {
                self.cmp(&ops[3], &ops[4]);
                self.env.a.cmov(
                    cc_of(imm(&ops[2])),
                    gpr(phys(&ops[0])),
                    gpr(phys(&ops[1])),
                    8,
                );
            }
            op::TESTCMOV => {
                let c = gpr(phys(&ops[3]));
                self.env.a.test_rr(c, c, 4);
                self.env.a.cmov(
                    cc_of(imm(&ops[2])),
                    gpr(phys(&ops[0])),
                    gpr(phys(&ops[1])),
                    8,
                );
            }
            op::CHK => {
                let d = gpr(phys(&ops[0]));
                let kind = imm(&ops[2]);
                let start = self.env.a.here();
                match (kind, &ops[1]) {
                    (op::CHK_MUL_S, Operand::Imm(n)) => self.env.a.imul_rri(d, d, *n as i32, 8),
                    (op::CHK_MUL_S, s) => self.env.a.imul_rr(d, gpr(phys(s)), 8),
                    (k, s) => {
                        let alu = if k == op::CHK_ADD_S || k == op::CHK_ADD_U {
                            Alu::Add
                        } else {
                            Alu::Sub
                        };
                        match s {
                            Operand::Imm(n) => self.env.a.alu_ri(alu, d, *n as i32, 8),
                            m @ Operand::Mem { .. } => {
                                let m = self.mem(m);
                                self.env.a.alu_rm(alu, d, m, 8);
                            }
                            s => self.env.a.alu_rr(alu, d, gpr(phys(s)), 8),
                        }
                    }
                }
                self.env.a.guard_jump(start, 6);
                let cc = if kind <= op::CHK_MUL_S { Cc::O } else { Cc::B };
                let l = self.env.stops[SYM_OVERFLOW as usize];
                self.ov_jump(cc, l, &ops[3..]);
            }
            op::MULU_CHK => {
                self.env.a.mul(gpr(phys(&ops[2])), 8);
                let l = self.env.stops[SYM_OVERFLOW as usize];
                self.ov_jump(Cc::B, l, &ops[3..]);
            }
            op::DIV => {
                let s = gpr(phys(&ops[2]));
                let kind = imm(&ops[3]);
                let (signed, rem, wide) = (
                    kind & op::DIV_SIGNED != 0,
                    kind & op::DIV_REM != 0,
                    kind & op::DIV_WIDE != 0,
                );
                self.env.a.test_rr(s, s, 8);
                let z = self.env.stops[SYM_DIV0 as usize];
                self.env.a.jcc(Cc::E, z);
                if signed {
                    let (normal, done) = (self.env.a.new_label(), self.env.a.new_label());
                    if wide {
                        // `MIN / -1` would trap: the interpreter wraps.
                        self.env.a.alu_ri(Alu::Cmp, s, -1, 8);
                        self.env.a.jcc(Cc::Ne, normal);
                        if rem {
                            self.env.a.alu_rr(Alu::Xor, x86::RDX, x86::RDX, 4);
                        } else {
                            self.env.a.neg(x86::RAX, 8);
                        }
                        self.env.a.jmp(done);
                    }
                    self.env.a.bind(normal);
                    self.env.a.sign_extend_acc(8);
                    self.env.a.idiv(s, 8);
                    self.env.a.bind(done);
                } else {
                    self.env.a.alu_rr(Alu::Xor, x86::RDX, x86::RDX, 4);
                    self.env.a.div(s, 8);
                }
            }
            op::MULHI => {
                let s = gpr(phys(&ops[2]));
                if imm(&ops[3]) == 1 {
                    self.env.a.imul1(s, 8);
                } else {
                    self.env.a.mul(s, 8);
                }
            }
            op::SHIFT_CL => {
                let d = gpr(phys(&ops[0]));
                let w = imm(&ops[3]) as i32;
                self.env.a.alu_ri(Alu::Cmp, x86::RCX, w, 8);
                let l = self.env.stops[SYM_SHIFT as usize];
                self.env.a.jcc(Cc::Ae, l);
                let sh = match imm(&ops[2]) {
                    0 => Shift::Shl,
                    1 => Shift::Shr,
                    _ => Shift::Sar,
                };
                self.env.a.shift_cl(sh, d, 8);
            }
            op::CALL => {
                let Operand::Sym(n) = ops[0] else {
                    panic!("a call without a function")
                };
                let l = self.env.funcs[(n - SYM_FUNC) as usize];
                self.env.a.call(l);
            }
            op::CALLR => self.env.a.call_reg(gpr(phys(&ops[0]))),
            op::CALLRT => {
                let rt = self.env.rt;
                let l = match imm(&ops[0]) {
                    op::RT_ALLOC => rt.alloc,
                    op::RT_FREE => rt.free,
                    op::RT_REALLOC => rt.realloc,
                    _ => rt.print,
                };
                self.env.a.call(l);
            }
            op::RET => self.epilogue(),
            op::STOP => {
                let Operand::Imm(n) = ops[0] else {
                    panic!("a stop without a message")
                };
                let msg = &self.strings[n as usize];
                let off = self.env.ro.add(msg.as_bytes(), 1);
                self.env.a.lea_sect(x86::RDI, Sect::Rodata, off);
                self.env.a.mov_ri(x86::RSI, msg.len() as i64);
                let stop = self.env.rt.stop;
                self.env.a.call(stop);
            }
            op::JMPSYM => {
                let l = self.stop_label(&ops[0]);
                self.env.a.jmp(l);
            }
            op::ZERO => {
                let Operand::Frame(id, disp) = ops[0] else {
                    panic!("zero wants a frame object")
                };
                self.zero(id, disp, imm(&ops[1]) as i32);
            }
            op::MEMCOPY => {
                let (to, from) = (self.mem(&ops[0]), self.mem(&ops[1]));
                self.copy_bytes(to, from, imm(&ops[2]) as i32);
            }
            op::REP_MOVSB => self.env.a.rep_movsb(),
            op::DATAADDR => {
                let off = self.env.data_off[imm(&ops[1]) as usize];
                self.env.a.lea_sect(gpr(phys(&ops[0])), Sect::Data, off);
            }
            op::FUNCADDR => {
                let Operand::Sym(n) = ops[1] else {
                    panic!("funcaddr without a function")
                };
                let l = self.env.funcs[(n - SYM_FUNC) as usize];
                self.env.a.lea_label(gpr(phys(&ops[0])), l);
            }
            op::CMPSTOP => {
                self.cmp(&ops[1], &ops[2]);
                let l = self.stop_label(&ops[3]);
                self.ov_jump(cc_of(imm(&ops[0])), l, &ops[4..]);
            }
            op::FLOAD => {
                let m = self.mem(&ops[1]);
                self.env.a.fload(xr(phys(&ops[0])), m, true);
            }
            op::FSTORE => {
                let m = self.mem(&ops[0]);
                self.env.a.fstore(m, xr(phys(&ops[1])), true);
            }
            op::VLOAD => {
                let m = self.mem(&ops[1]);
                self.env.a.movups_load(xr(phys(&ops[0])), m);
            }
            op::VSTORE => {
                let m = self.mem(&ops[0]);
                self.env.a.movups_store(m, xr(phys(&ops[1])));
            }
            op::FCONST => {
                let d = xr(phys(&ops[0]));
                match imm(&ops[1]) {
                    0 => self.env.a.fxor(d, d),
                    n => {
                        self.env.a.mov_ri(x86::RAX, n);
                        self.env.a.movq_to_xmm(d, x86::RAX, true);
                    }
                }
            }
            op::FLD => {
                let m = self.mem(&ops[1]);
                self.env.a.fload(xr(phys(&ops[0])), m, imm(&ops[2]) == 1);
            }
            op::FST => {
                let m = self.mem(&ops[0]);
                self.env.a.fstore(m, xr(phys(&ops[1])), imm(&ops[2]) == 1);
            }
            op::FARITH => {
                let sse = [Sse::Add, Sse::Sub, Sse::Mul, Sse::Div][imm(&ops[2]) as usize];
                self.env
                    .a
                    .farith(sse, xr(phys(&ops[0])), xr(phys(&ops[1])), imm(&ops[3]) == 1);
            }
            op::FNEG => {
                let d = xr(phys(&ops[0]));
                let m = x86::Xmm(15);
                if imm(&ops[1]) == 1 {
                    self.env.a.mov_ri(x86::RAX, i64::MIN);
                    self.env.a.movq_to_xmm(m, x86::RAX, true);
                } else {
                    self.env.a.mov_ri(x86::RAX, 0x8000_0000);
                    self.env.a.movq_to_xmm(m, x86::RAX, false);
                }
                self.env.a.fxor(d, m);
            }
            op::FCMPSET => {
                let d = gpr(phys(&ops[0]));
                let dbl = imm(&ops[4]) == 1;
                self.env.a.fcmp(xr(phys(&ops[2])), xr(phys(&ops[3])), dbl);
                match imm(&ops[1]) {
                    // Equal needs the zero flag set and the parity flag clear (unordered sets both).
                    op::FCMP_EQ => {
                        self.env.a.setcc(Cc::E, d);
                        self.env.a.setcc(Cc::Np, x86::RAX);
                        self.env.a.alu_rr(Alu::And, d, x86::RAX, 1);
                    }
                    op::FCMP_NE => {
                        self.env.a.setcc(Cc::Ne, d);
                        self.env.a.setcc(Cc::P, x86::RAX);
                        self.env.a.alu_rr(Alu::Or, d, x86::RAX, 1);
                    }
                    n => self.env.a.setcc(cc_of(n), d),
                }
                self.env.a.extend_rr(d, d, 1, false, false);
            }
            op::FCMPJCC => {
                let start = self.env.a.here();
                self.env
                    .a
                    .fcmp(xr(phys(&ops[1])), xr(phys(&ops[2])), imm(&ops[4]) == 1);
                self.env.a.guard_jump(start, 6);
                let l = self.block_label(&ops[3]);
                self.env.a.jcc(cc_of(imm(&ops[0])), l);
            }
            op::ITOF => {
                let d = xr(phys(&ops[0]));
                // The conversion keeps the rest of its destination: clear it to break the dependency.
                self.env.a.fxor(d, d);
                self.env
                    .a
                    .cvt_int_to_float(d, gpr(phys(&ops[1])), true, imm(&ops[2]) == 1);
            }
            op::FTOI => {
                let (d, x) = (gpr(phys(&ops[0])), xr(phys(&ops[1])));
                let double = imm(&ops[2]) == 1;
                self.env.a.cvt_float_to_int(d, x, true, double);
                // `cvttsd2si` answers `i64::MIN` for NaN and anything out of range. Saturate like
                // Rust's `as`: NaN gives 0, a positive overflow gives `i64::MAX`.
                let (done, nan, big) = (
                    self.env.a.new_label(),
                    self.env.a.new_label(),
                    self.env.a.new_label(),
                );
                self.env.a.mov_ri(x86::RAX, i64::MIN);
                self.env.a.alu_rr(Alu::Cmp, d, x86::RAX, 8);
                self.env.a.jcc(Cc::Ne, done);
                let z = x86::Xmm(15);
                self.env.a.fxor(z, z);
                self.env.a.fcmp(x, z, double);
                self.env.a.jcc(Cc::P, nan);
                self.env.a.jcc(Cc::A, big);
                self.env.a.jmp(done);
                self.env.a.bind(nan);
                self.env.a.alu_rr(Alu::Xor, d, d, 4);
                self.env.a.jmp(done);
                self.env.a.bind(big);
                self.env.a.mov_ri(d, i64::MAX);
                self.env.a.bind(done);
                self.mask(d, imm(&ops[3]));
            }
            op::FRESIZE => {
                let (d, x) = (xr(phys(&ops[0])), xr(phys(&ops[1])));
                self.env.a.cvt_float_resize(d, x, imm(&ops[2]) == 1);
            }
            op::FFMA => {
                let dbl = imm(&ops[3]) == 1;
                let b = self.fmem_op(&ops[2]);
                self.env
                    .a
                    .vfmadd231(xr(phys(&ops[0])), xr(phys(&ops[1])), b, dbl);
            }
            op::FREM => {
                // x87 `fprem` is exactly C's `fmod`: the sign of the dividend, NaN for a zero divisor.
                // It reads memory: the operands wait in a frame object.
                let Operand::Frame(id, disp) = ops[3] else {
                    panic!("frem wants a frame object")
                };
                let dbl = imm(&ops[4]) == 1;
                let (ma, mb) = (self.frame_mem(id, disp), self.frame_mem(id, disp + 8));
                self.env.a.fstore(ma, xr(phys(&ops[1])), dbl);
                self.env.a.fstore(mb, xr(phys(&ops[2])), dbl);
                self.env.a.x87_load(mb, dbl);
                self.env.a.x87_load(ma, dbl);
                let again = self.env.a.new_label();
                self.env.a.bind(again);
                self.env.a.x87_prem();
                self.env.a.x87_status_ax();
                self.env.a.shift_imm(Shift::Shr, x86::RAX, 10, 4);
                self.env.a.alu_ri(Alu::And, x86::RAX, 1, 4);
                self.env.a.jcc(Cc::Ne, again);
                self.env.a.x87_drop_second();
                self.env.a.x87_store_pop(ma, dbl);
                self.env.a.fload(xr(phys(&ops[0])), ma, dbl);
            }
            op::VSPLAT => {
                let d = xr(phys(&ops[0]));
                self.env.a.movq_to_xmm(d, gpr(phys(&ops[1])), true);
                self.env.a.pshufd(d, d, 0x44);
            }
            op::VSHIFT => {
                self.env
                    .a
                    .vshift(imm(&ops[1]) == 1, xr(phys(&ops[0])), imm(&ops[2]) as u8);
            }
            op::VREDUCE => {
                let (s, t) = (xr(phys(&ops[1])), x86::Xmm(15));
                self.env.a.pshufd(t, s, 0xEE);
                self.env.a.valu(VAlu::Add, t, s);
                self.env.a.movq_from_xmm(gpr(phys(&ops[0])), t, true);
            }
            op::VBUILD => {
                let (d, t) = (xr(phys(&ops[0])), x86::Xmm(15));
                self.env.a.movq_to_xmm(d, gpr(phys(&ops[1])), true);
                self.env.a.movq_to_xmm(t, gpr(phys(&ops[2])), true);
                self.env.a.punpcklqdq(d, t);
            }
            op::VALU => {
                let k =
                    [VAlu::Add, VAlu::Sub, VAlu::And, VAlu::Or, VAlu::Xor][imm(&ops[2]) as usize];
                self.env.a.valu(k, xr(phys(&ops[0])), xr(phys(&ops[1])));
            }
            op::REP_STOS => self.env.a.rep_stos(imm(&ops[3]) as u8),
            o => panic!("the x86-64 emitter does not know opcode {o}"),
        }
    }
}

/// Emits the function; `label` is bound at its first byte.
pub fn emit(env: &mut Env, f: &MFunc, label: Label) {
    let fr = layout(f);
    let labels: Vec<Label> = f.blocks.iter().map(|_| env.a.new_label()).collect();
    let mut em = Em {
        env,
        fr,
        labels,
        strings: &f.strings,
        cold: Vec::new(),
    };
    em.env.a.bind(label);
    em.prologue();
    for (bi, b) in f.blocks.iter().enumerate() {
        // A loop starts on a 16-byte boundary (when that costs at most 15 bytes of no-ops: a loop whose head
        // sat 9 to 15 bytes past a boundary ran 30 percent slower than the same loop aligned).
        if b.loop_head {
            em.env.a.align(16, 15);
        }
        let l = em.labels[bi];
        em.env.a.bind(l);
        for i in &b.insts {
            em.inst(i);
        }
    }
    em.cold_stubs();
}
