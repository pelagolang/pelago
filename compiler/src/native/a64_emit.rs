//! From allocated AArch64 MIR to bytes (design in `docs/design/mir.md`).
//!
//! The frame has no frame pointer. From `sp` up: the outgoing argument area, the saved registers (`x30`
//! when the function calls, then the callee-saved ones it uses), the other frame objects. The stack
//! pointer moves once, in the prologue, and stays: every frame object is `[sp + k]` and the arguments the
//! caller left on the stack are at `[sp + size + 8 * i]`. `x16` and `x17` belong to the assembler's own
//! helpers (`a64.rs`) and to the address of an indexed access; `x9` to `x12` are the allocator's, and a
//! macro that needs one of them says so in `clobbers`.

use super::a64::{self as hw, Asm, Cond, Label, Sect, Shift};
use super::a64_mir::{cc_of, op, SYM_FUNC};
use super::elf::Blob;
use super::mir::*;
use super::rt_a64::Runtime;

/// What the emitter takes from the generator.
pub struct Env<'a> {
    pub a: &'a mut Asm,
    pub ro: &'a mut Blob,
    pub funcs: &'a [Label],
    pub data_off: &'a [u32],
    /// The exits of this function that stop the program, by `SYM_*` number (emitted at its end when used).
    pub stops: [Label; 4],
    pub rt: &'a Runtime,
}

const STOP_MESSAGES: [&str; 4] = [
    "division by zero",
    "shifted by the width or more",
    "arithmetic overflow",
    "reached unreachable code",
];

fn gp(p: PReg) -> hw::Reg {
    debug_assert!(p.0 < 32, "{p:?} is not a general register");
    hw::Reg(p.0)
}

fn vr(p: PReg) -> hw::V {
    debug_assert!(p.0 >= 32, "{p:?} is not a vector register");
    hw::V(p.0 - 32)
}

fn is_vec(p: PReg) -> bool {
    p.0 >= 32
}

fn phys(o: &Operand) -> PReg {
    match o {
        Operand::Reg(Reg::Phys(p), _) => *p,
        o => panic!("an unallocated or missing register: {o:?}"),
    }
}

fn r(o: &Operand) -> hw::Reg {
    gp(phys(o))
}

fn imm(o: &Operand) -> i64 {
    match o {
        Operand::Imm(n) => *n,
        o => panic!("an immediate was expected: {o:?}"),
    }
}

/// A saved register.
#[derive(Clone, Copy)]
enum Save {
    X(hw::Reg),
    D(hw::V),
}

struct Frame {
    /// Offset of each object from `sp` (the incoming arguments from `sp` too).
    off: Vec<i32>,
    saves: Vec<Save>,
    save_at: i32,
    /// The `sub sp` amount, a multiple of 16.
    size: i32,
}

fn layout(f: &MFunc) -> Frame {
    let mut off = vec![0i32; f.frame.len()];
    let mut at = 0i32;
    let place = |i: usize, at: &mut i32, off: &mut Vec<i32>| {
        let o = &f.frame[i];
        let al = (o.align as i32).max(1);
        *at = (*at + al - 1) / al * al;
        off[i] = *at;
        *at += o.size as i32;
    };
    // The outgoing arguments first: they sit at `[sp]`.
    for i in 0..f.frame.len() {
        if f.frame[i].kind == FrameKind::Outgoing {
            place(i, &mut at, &mut off);
        }
    }
    let calls = f
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .any(|i| i.flags.has(IFlags::CALL));
    let mut saves = Vec::new();
    if calls {
        saves.push(Save::X(hw::LR));
    }
    for p in f.used_callee_saved.iter() {
        saves.push(if is_vec(p) {
            Save::D(vr(p))
        } else {
            Save::X(gp(p))
        });
    }
    at = (at + 7) / 8 * 8;
    let save_at = at;
    at += 8 * saves.len() as i32;
    for i in 0..f.frame.len() {
        if !matches!(f.frame[i].kind, FrameKind::Outgoing | FrameKind::Incoming) {
            place(i, &mut at, &mut off);
        }
    }
    let size = (at + 15) / 16 * 16;
    for (i, o) in f.frame.iter().enumerate() {
        if o.kind == FrameKind::Incoming {
            off[i] = size;
        }
    }
    Frame {
        off,
        saves,
        save_at,
        size,
    }
}

/// The out-of-line half of a stop that carries a test (a `CheckedIf`): after the overflow branch, stop
/// when the test holds, else go back to `resume`. Registers are as they were at the branch.
struct Cold {
    label: Label,
    resume: Label,
    stop: Label,
    test: Vec<Operand>,
}

/// The branch taken on overflow.
enum Br {
    Cc(Cond),
    Nz(hw::Reg),
}

/// A memory access: base plus offset, or base plus register.
enum Ea {
    Off(hw::Reg, i32),
    Idx(hw::Reg, hw::Reg, bool),
}

struct Em<'e, 'a> {
    env: &'e mut Env<'a>,
    fr: Frame,
    labels: Vec<Label>,
    strings: &'e [String],
    cold: Vec<Cold>,
    used: [bool; 4],
}

impl Em<'_, '_> {
    fn a(&mut self) -> &mut Asm {
        self.env.a
    }

    fn block_label(&self, o: &Operand) -> Label {
        match o {
            Operand::Block(b) => self.labels[*b as usize],
            o => panic!("a block was expected: {o:?}"),
        }
    }

    fn stop(&mut self, n: u32) -> Label {
        self.used[n as usize] = true;
        self.env.stops[n as usize]
    }

    fn stop_label(&mut self, o: &Operand) -> Label {
        match o {
            Operand::Sym(n) => self.stop(*n),
            o => panic!("a stop routine was expected: {o:?}"),
        }
    }

    // ---- Frame -------------------------------------------------------------------------------------

    fn saved(&mut self, load: bool) {
        let saves = std::mem::take(&mut self.fr.saves);
        let at = self.fr.save_at;
        let mut k = 0;
        while k < saves.len() {
            let off = at + 8 * k as i32;
            match (saves[k], saves.get(k + 1)) {
                (Save::X(x), Some(&Save::X(y))) if off + 8 <= 504 => {
                    if load {
                        self.a().ldp(x, y, hw::SP, off);
                    } else {
                        self.a().stp(x, y, hw::SP, off);
                    }
                    k += 2;
                }
                (Save::X(x), _) => {
                    if load {
                        self.a().ldr(x, hw::SP, off, 8);
                    } else {
                        self.a().str(x, hw::SP, off, 8);
                    }
                    k += 1;
                }
                (Save::D(v), _) => {
                    if load {
                        self.a().ldr_f(v, hw::SP, off, 8);
                    } else {
                        self.a().str_f(v, hw::SP, off, 8);
                    }
                    k += 1;
                }
            }
        }
        self.fr.saves = saves;
    }

    fn prologue(&mut self) {
        let size = self.fr.size;
        if size > 0 {
            // A big frame must not step over the guard: touch each page, top down, first.
            let pages = size / 4096;
            let a = &mut *self.env.a;
            if pages <= 8 {
                for k in 1..=pages {
                    a.sub_ri(hw::X16, hw::SP, 4096 * k as u32);
                    a.ldr(hw::XZR, hw::X16, 0, 8);
                }
            } else {
                // A loop, not a probe per page in the code: a 40 MB array is 10 000 of them.
                let top = a.new_label();
                a.mov_from_sp(hw::X16);
                a.add_imm(hw::X17, hw::SP, -(4096 * pages as i64));
                a.bind(top);
                a.sub_ri(hw::X16, hw::X16, 4096);
                a.ldr(hw::XZR, hw::X16, 0, 8);
                a.cmp_rr(hw::X16, hw::X17);
                a.b_cond(Cond::Hi, top);
            }
            a.add_imm(hw::SP, hw::SP, -(size as i64));
        }
        self.saved(false);
    }

    fn epilogue(&mut self) {
        self.saved(true);
        let size = self.fr.size;
        if size > 0 {
            self.a().add_imm(hw::SP, hw::SP, size as i64);
        }
        self.a().ret();
    }

    // ---- Operands ----------------------------------------------------------------------------------

    fn ea(&mut self, o: &Operand, size: u8) -> Ea {
        let Operand::Mem { base, index, disp } = o else {
            panic!("a memory operand was expected: {o:?}")
        };
        let (b, extra) = match base {
            MemBase::Reg(Reg::Phys(p)) => (gp(*p), 0),
            MemBase::Frame(id) => (hw::SP, self.fr.off[id.0 as usize]),
            MemBase::Reg(x) => panic!("an unallocated register {x:?}"),
        };
        let disp = disp + extra;
        let Some((ix, scale)) = index else {
            return Ea::Off(b, disp);
        };
        let Reg::Phys(ix) = ix else {
            panic!("an unallocated register {ix:?}")
        };
        let i = gp(*ix);
        if disp == 0 {
            if *scale == 1 {
                return Ea::Idx(b, i, false);
            }
            if *scale == size {
                return Ea::Idx(b, i, true);
            }
        }
        let sh = scale.trailing_zeros();
        let a = &mut *self.env.a;
        if b == hw::SP {
            if sh > 0 {
                a.lsl_ri(hw::X17, i, sh);
                a.add_ext(hw::X17, hw::SP, hw::X17);
            } else {
                a.add_ext(hw::X17, hw::SP, i);
            }
        } else {
            a.add_rr_sh(hw::X17, b, i, Shift::Lsl, sh);
        }
        Ea::Off(hw::X17, disp)
    }

    /// `dst = src` for registers, immediates and frame addresses.
    fn copy(&mut self, d: PReg, s: &Operand) {
        match s {
            Operand::Reg(Reg::Phys(s), _) => {
                if d == *s {
                    return;
                }
                if is_vec(d) {
                    self.a().fmov_rr(true, vr(d), vr(*s));
                } else {
                    self.a().mov_rr(gp(d), gp(*s));
                }
            }
            Operand::Imm(n) => self.a().mov_ri(gp(d), *n),
            Operand::Frame(id, disp) => {
                let off = self.fr.off[id.0 as usize] + disp;
                self.a().add_imm(gp(d), hw::SP, off as i64);
            }
            o => panic!("copy from {o:?}"),
        }
    }

    /// `d = a op b` for `add`, `sub`, `and`, `orr`, `eor`.
    fn alu(&mut self, opc: u16, ops: &[Operand]) {
        let (d, x) = (r(&ops[0]), r(&ops[1]));
        match &ops[2] {
            Operand::Imm(n) => {
                let n = *n;
                let a = &mut *self.env.a;
                match opc {
                    op::ADD => a.add_imm(d, x, n),
                    op::SUB => a.add_imm(d, x, n.wrapping_neg()),
                    _ => {
                        let done = match opc {
                            op::AND => a.and_ri(d, x, n as u64),
                            op::OR => a.orr_ri(d, x, n as u64),
                            _ => a.eor_ri(d, x, n as u64),
                        };
                        if !done {
                            a.mov_ri(hw::X17, n);
                            match opc {
                                op::AND => a.and_rr(d, x, hw::X17),
                                op::OR => a.orr_rr(d, x, hw::X17),
                                _ => a.eor_rr(d, x, hw::X17),
                            }
                        }
                    }
                }
            }
            o => {
                let y = r(o);
                let a = &mut *self.env.a;
                match opc {
                    op::ADD => a.add_rr(d, x, y),
                    op::SUB => a.sub_rr(d, x, y),
                    op::AND => a.and_rr(d, x, y),
                    op::OR => a.orr_rr(d, x, y),
                    _ => a.eor_rr(d, x, y),
                }
            }
        }
    }

    /// `cmp a, b` where `b` is a register or an immediate.
    fn cmp(&mut self, x: &Operand, y: &Operand) {
        let rx = r(x);
        match y {
            Operand::Imm(n) if Asm::fits_addsub(*n as u64) => self.a().cmp_ri(rx, *n as u32),
            Operand::Imm(n) => {
                let a = &mut *self.env.a;
                a.mov_ri(hw::X17, *n);
                a.cmp_rr(rx, hw::X17);
            }
            y => self.a().cmp_rr(rx, r(y)),
        }
    }

    // ---- Stops -------------------------------------------------------------------------------------

    /// The branch taken on overflow. An instruction that carries a test (`tail`, from a `CheckedIf`)
    /// branches to a cold stub that stops only when the test holds.
    fn ov_branch(&mut self, br: Br, stop: Label, tail: &[Operand]) {
        let (label, resume) = if tail.is_empty() {
            (stop, None)
        } else {
            let (l, r) = (self.a().new_label(), self.a().new_label());
            (l, Some(r))
        };
        match br {
            Br::Cc(cc) => self.a().b_cond(cc, label),
            Br::Nz(x) => self.a().cbnz(x, label),
        }
        if let Some(resume) = resume {
            self.a().bind(resume);
            self.cold.push(Cold {
                label,
                resume,
                stop,
                test: tail.to_vec(),
            });
        }
    }

    fn cold_stubs(&mut self) {
        for c in std::mem::take(&mut self.cold) {
            self.a().bind(c.label);
            match imm(&c.test[0]) {
                // `[0, cond, when]`: stop when the bool equals `when`.
                0 => {
                    let x = r(&c.test[1]);
                    if imm(&c.test[2]) == 1 {
                        self.a().cbz(x, c.resume);
                    } else {
                        self.a().cbnz(x, c.resume);
                    }
                }
                // `[1, cc, a, b]`: stop when the comparison holds.
                _ => {
                    self.cmp(&c.test[2], &c.test[3]);
                    self.a().b_cond(cc_of(imm(&c.test[1])).invert(), c.resume);
                }
            }
            self.a().b(c.stop);
        }
    }

    fn stop_routines(&mut self) {
        for (k, msg) in STOP_MESSAGES.iter().enumerate() {
            if !self.used[k] {
                continue;
            }
            let off = self.env.ro.add(msg.as_bytes(), 1);
            let l = self.env.stops[k];
            let stop = self.env.rt.stop;
            let a = &mut *self.env.a;
            a.bind(l);
            a.lea_sect(hw::X0, Sect::Rodata, off);
            a.mov_ri(hw::X1, msg.len() as i64);
            a.bl(stop);
        }
    }

    // ---- Macros ------------------------------------------------------------------------------------

    fn zero(&mut self, id: FrameId, disp: i32, size: i32) {
        let off = self.fr.off[id.0 as usize] + disp;
        let a = &mut *self.env.a;
        let mut k = 0;
        if size <= 128 {
            while size - k >= 16 && (off + k) % 8 == 0 && off + k >= 0 && off + k + 8 <= 504 {
                a.stp(hw::XZR, hw::XZR, hw::SP, off + k);
                k += 16;
            }
            for w in [8, 4, 2, 1] {
                while size - k >= w {
                    a.str(hw::XZR, hw::SP, off + k, w as u8);
                    k += w;
                }
            }
        } else {
            // x9 walks, x10 is the end of the 16-byte part.
            let top = a.new_label();
            a.add_imm(hw::X9, hw::SP, off as i64);
            a.add_imm(hw::X10, hw::X9, (size & !15) as i64);
            a.bind(top);
            a.stp(hw::XZR, hw::XZR, hw::X9, 0);
            a.add_ri(hw::X9, hw::X9, 16);
            a.cmp_rr(hw::X9, hw::X10);
            a.b_cond(Cond::Lo, top);
            let mut k = 0;
            for w in [8, 4, 2, 1] {
                while (size & 15) - k >= w {
                    a.str(hw::XZR, hw::X9, k, w as u8);
                    k += w;
                }
            }
        }
    }

    fn mem_copy(&mut self, to: &Operand, from: &Operand, len: i32) {
        let (Ea::Off(tb, to), Ea::Off(fb, from)) = (self.ea(to, 8), self.ea(from, 8)) else {
            panic!("a copy between indexed addresses")
        };
        let a = &mut *self.env.a;
        let mut k = 0;
        for w in [8, 4, 2, 1] {
            while len - k >= w {
                a.ldr(hw::X9, fb, from + k, w as u8);
                a.str(hw::X9, tb, to + k, w as u8);
                k += w;
            }
        }
    }

    fn mask(&mut self, d: hw::Reg, s: hw::Reg, bits: i64) {
        match bits {
            8 | 16 | 32 => self.a().uxt(d, s, bits as u32),
            _ => {
                if d != s {
                    self.a().mov_rr(d, s);
                }
            }
        }
    }

    fn chk(&mut self, ops: &[Operand]) {
        let (d, x) = (r(&ops[0]), r(&ops[1]));
        let kind = imm(&ops[3]);
        let tail = &ops[4..];
        let ov = self.stop(super::a64_mir::SYM_OVERFLOW);
        match kind {
            op::CHK_ADD_S | op::CHK_SUB_S | op::CHK_ADD_U | op::CHK_SUB_U => {
                let add = kind == op::CHK_ADD_S || kind == op::CHK_ADD_U;
                let a = &mut *self.env.a;
                match (&ops[2], add) {
                    (Operand::Imm(n), true) => a.adds_ri(d, x, *n as u32),
                    (Operand::Imm(n), false) => a.subs_ri(d, x, *n as u32),
                    (y, true) => a.adds_rr(d, x, r(y)),
                    (y, false) => a.subs_rr(d, x, r(y)),
                }
                let cc = match kind {
                    op::CHK_ADD_S | op::CHK_SUB_S => Cond::Vs,
                    op::CHK_ADD_U => Cond::Hs,
                    _ => Cond::Lo,
                };
                self.ov_branch(Br::Cc(cc), ov, tail);
            }
            op::CHK_MUL_S => {
                // The high half first: `d` may be one of the operands.
                let y = r(&ops[2]);
                let a = &mut *self.env.a;
                a.smulh(hw::X9, x, y);
                a.mul(d, x, y);
                a.cmp_rr_sh(hw::X9, d, Shift::Asr, 63);
                self.ov_branch(Br::Cc(Cond::Ne), ov, tail);
            }
            op::CHK_MUL_U => {
                let y = r(&ops[2]);
                let a = &mut *self.env.a;
                a.umulh(hw::X9, x, y);
                a.mul(d, x, y);
                self.ov_branch(Br::Nz(hw::X9), ov, tail);
            }
            k => panic!("check kind {k}"),
        }
    }

    fn div(&mut self, ops: &[Operand]) {
        let (d, x, y) = (r(&ops[0]), r(&ops[1]), r(&ops[2]));
        let kind = imm(&ops[3]);
        let (signed, rem) = (kind & op::DIV_SIGNED != 0, kind & op::DIV_REM != 0);
        let z = self.stop(super::a64_mir::SYM_DIV0);
        let a = &mut *self.env.a;
        a.cbz(y, z);
        // `MIN / -1` does not trap here: the quotient wraps, like the interpreter's.
        let q = if rem { hw::X9 } else { d };
        if signed {
            a.sdiv(q, x, y);
        } else {
            a.udiv(q, x, y);
        }
        if rem {
            a.msub(d, hw::X9, y, x);
        }
    }

    fn inst(&mut self, i: &MInst, next: Option<u32>) {
        let ops = &i.ops;
        match i.op {
            COPY => {
                let d = phys(&ops[0]);
                self.copy(d, &ops[1]);
            }
            op::ADD | op::SUB | op::AND | op::OR | op::XOR => self.alu(i.op, ops),
            op::MUL => self.a().mul(r(&ops[0]), r(&ops[1]), r(&ops[2])),
            op::MADD => self
                .a()
                .madd(r(&ops[0]), r(&ops[1]), r(&ops[2]), r(&ops[3])),
            op::MSUB => self
                .a()
                .msub(r(&ops[0]), r(&ops[1]), r(&ops[2]), r(&ops[3])),
            op::LEA => {
                let d = r(&ops[0]);
                let Operand::Mem { base, index, disp } = &ops[1] else {
                    panic!("lea wants a memory operand")
                };
                let (b, extra) = match base {
                    MemBase::Reg(Reg::Phys(p)) => (gp(*p), 0),
                    MemBase::Frame(id) => (hw::SP, self.fr.off[id.0 as usize]),
                    MemBase::Reg(x) => panic!("an unallocated register {x:?}"),
                };
                let disp = (disp + extra) as i64;
                match index {
                    None => self.a().add_imm(d, b, disp),
                    Some((Reg::Phys(ix), s)) => {
                        let (ix, sh) = (gp(*ix), s.trailing_zeros());
                        let a = &mut *self.env.a;
                        if b == hw::SP {
                            if sh > 0 {
                                a.lsl_ri(hw::X17, ix, sh);
                                a.add_ext(d, hw::SP, hw::X17);
                            } else {
                                a.add_ext(d, hw::SP, ix);
                            }
                        } else {
                            a.add_rr_sh(d, b, ix, Shift::Lsl, sh);
                        }
                        if disp != 0 {
                            a.add_imm(d, d, disp);
                        }
                    }
                    Some((x, _)) => panic!("an unallocated register {x:?}"),
                }
            }
            op::NEG => self.a().neg(r(&ops[0]), r(&ops[1])),
            op::NOT => self.a().mvn(r(&ops[0]), r(&ops[1])),
            op::SHL_I => self.a().lsl_ri(r(&ops[0]), r(&ops[1]), imm(&ops[2]) as u32),
            op::SHR_I => self.a().lsr_ri(r(&ops[0]), r(&ops[1]), imm(&ops[2]) as u32),
            op::SAR_I => self.a().asr_ri(r(&ops[0]), r(&ops[1]), imm(&ops[2]) as u32),
            op::MASK => self.mask(r(&ops[0]), r(&ops[1]), imm(&ops[2])),
            op::EXT => {
                let (d, s) = (r(&ops[0]), r(&ops[1]));
                let bits = 8 * imm(&ops[2]) as u32;
                if bits >= 64 {
                    if d != s {
                        self.a().mov_rr(d, s);
                    }
                } else if imm(&ops[3]) == 1 {
                    self.a().sxt(d, s, bits);
                } else {
                    self.a().uxt(d, s, bits);
                }
            }
            op::LOAD => {
                let d = r(&ops[0]);
                let size = imm(&ops[2]) as u8;
                match self.ea(&ops[1], size) {
                    Ea::Off(b, off) => self.a().ldr(d, b, off, size),
                    Ea::Idx(b, x, scaled) => self.a().ldr_idx(d, b, x, size, scaled),
                }
            }
            op::STORE => {
                let size = imm(&ops[2]) as u8;
                let s = match &ops[1] {
                    Operand::Imm(0) => hw::XZR,
                    o => r(o),
                };
                match self.ea(&ops[0], size) {
                    Ea::Off(b, off) => self.a().str(s, b, off, size),
                    Ea::Idx(b, x, scaled) => self.a().str_idx(s, b, x, size, scaled),
                }
            }
            op::CMPJCC => {
                let l = self.block_label(&ops[3]);
                let cc = cc_of(imm(&ops[0]));
                match (&ops[2], cc) {
                    (Operand::Imm(0), Cond::Eq) => self.a().cbz(r(&ops[1]), l),
                    (Operand::Imm(0), Cond::Ne) => self.a().cbnz(r(&ops[1]), l),
                    _ => {
                        self.cmp(&ops[1], &ops[2]);
                        self.a().b_cond(cc, l);
                    }
                }
            }
            op::TESTJCC => {
                let l = self.block_label(&ops[2]);
                if cc_of(imm(&ops[0])) == Cond::Eq {
                    self.a().cbz(r(&ops[1]), l);
                } else {
                    self.a().cbnz(r(&ops[1]), l);
                }
            }
            op::JMP => {
                if let Operand::Block(b) = ops[0] {
                    if Some(b) == next {
                        return;
                    }
                }
                let l = self.block_label(&ops[0]);
                self.a().b(l);
            }
            op::CMPSET => {
                self.cmp(&ops[2], &ops[3]);
                self.a().cset(r(&ops[0]), cc_of(imm(&ops[1])));
            }
            op::CMPSEL => {
                self.cmp(&ops[4], &ops[5]);
                self.a()
                    .csel(r(&ops[0]), r(&ops[1]), r(&ops[2]), cc_of(imm(&ops[3])));
            }
            op::TESTSEL => {
                self.a().cmp_ri(r(&ops[3]), 0);
                self.a().csel(r(&ops[0]), r(&ops[1]), r(&ops[2]), Cond::Ne);
            }
            op::CHK => self.chk(ops),
            op::DIV => self.div(ops),
            op::MULHI => {
                let (d, x, y) = (r(&ops[0]), r(&ops[1]), r(&ops[2]));
                if imm(&ops[3]) == 1 {
                    self.a().smulh(d, x, y);
                } else {
                    self.a().umulh(d, x, y);
                }
            }
            op::SHIFT_CHK => {
                let (d, x, y) = (r(&ops[0]), r(&ops[1]), r(&ops[2]));
                let l = self.stop(super::a64_mir::SYM_SHIFT);
                let a = &mut *self.env.a;
                a.cmp_ri(y, imm(&ops[4]) as u32);
                a.b_cond(Cond::Hs, l);
                match imm(&ops[3]) {
                    0 => a.lslv(d, x, y),
                    1 => a.lsrv(d, x, y),
                    _ => a.asrv(d, x, y),
                }
            }
            op::CALL => {
                let Operand::Sym(n) = ops[0] else {
                    panic!("a call without a function")
                };
                let l = self.env.funcs[(n - SYM_FUNC) as usize];
                self.a().bl(l);
            }
            op::CALLR => self.a().blr(r(&ops[0])),
            op::CALLRT => {
                let rt = self.env.rt;
                let l = match imm(&ops[0]) {
                    op::RT_ALLOC => rt.alloc,
                    op::RT_FREE => rt.free,
                    op::RT_REALLOC => rt.realloc,
                    op::RT_CTX_INIT => rt.ctx_init,
                    op::RT_CTX_SWITCH => rt.ctx_switch,
                    op::RT_MEMCPY => rt.memcpy,
                    op::RT_FMOD => rt.fmod,
                    _ => rt.print,
                };
                self.a().bl(l);
            }
            op::RET => self.epilogue(),
            op::STOP => {
                let Operand::Imm(n) = ops[0] else {
                    panic!("a stop without a message")
                };
                let msg = &self.strings[n as usize];
                let off = self.env.ro.add(msg.as_bytes(), 1);
                let len = msg.len() as i64;
                let stop = self.env.rt.stop;
                let a = &mut *self.env.a;
                a.lea_sect(hw::X0, Sect::Rodata, off);
                a.mov_ri(hw::X1, len);
                a.bl(stop);
            }
            op::JMPSYM => {
                let l = self.stop_label(&ops[0]);
                self.a().b(l);
            }
            op::ZERO => {
                let Operand::Frame(id, disp) = ops[0] else {
                    panic!("zero wants a frame object")
                };
                self.zero(id, disp, imm(&ops[1]) as i32);
            }
            op::MEMCOPY => self.mem_copy(&ops[0], &ops[1], imm(&ops[2]) as i32),
            op::DATAADDR => {
                let off = self.env.data_off[imm(&ops[1]) as usize];
                self.a().lea_sect(r(&ops[0]), Sect::Data, off);
            }
            op::FUNCADDR => {
                let Operand::Sym(n) = ops[1] else {
                    panic!("funcaddr without a function")
                };
                let l = self.env.funcs[(n - SYM_FUNC) as usize];
                self.a().lea_label(r(&ops[0]), l);
            }
            op::CMPSTOP => {
                self.cmp(&ops[1], &ops[2]);
                let l = self.stop_label(&ops[3]);
                self.ov_branch(Br::Cc(cc_of(imm(&ops[0]))), l, &ops[4..]);
            }
            o => panic!("the AArch64 emitter does not know opcode {o}"),
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
        used: [false; 4],
    };
    em.a().bind(label);
    em.prologue();
    for (bi, b) in f.blocks.iter().enumerate() {
        let l = em.labels[bi];
        em.a().bind(l);
        let next = (bi + 1 < f.blocks.len()).then_some(bi as u32 + 1);
        for i in &b.insts {
            em.inst(i, next);
        }
    }
    em.cold_stubs();
    em.stop_routines();
}
