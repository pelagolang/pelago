//! An x86-64 instruction encoder: only the forms the code generator uses (card A-09).
//!
//! `Asm` collects bytes and labels. Jumps and calls to labels are always `rel32`; references to
//! the data sections are patched when the final layout is known ([`Asm::finish`]). Every encoding
//! is checked against the system assembler by the tests (golden bytes, plus a live check when `as`
//! is installed).

/// A general-purpose register, numbered as the hardware numbers them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Reg(pub u8);

pub const RAX: Reg = Reg(0);
pub const RCX: Reg = Reg(1);
pub const RDX: Reg = Reg(2);
pub const RBX: Reg = Reg(3);
pub const RSP: Reg = Reg(4);
pub const RBP: Reg = Reg(5);
pub const RSI: Reg = Reg(6);
pub const RDI: Reg = Reg(7);
pub const R8: Reg = Reg(8);
pub const R9: Reg = Reg(9);
pub const R10: Reg = Reg(10);
pub const R11: Reg = Reg(11);
pub const R12: Reg = Reg(12);
pub const R13: Reg = Reg(13);
pub const R14: Reg = Reg(14);
pub const R15: Reg = Reg(15);

/// An SSE register.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Xmm(pub u8);

/// A memory operand: `[base + index * scale + disp]`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Mem {
    pub base: Reg,
    /// A register and its scale (1, 2, 4 or 8); never `rsp`.
    pub index: Option<(Reg, u8)>,
    pub disp: i32,
}

pub const fn mem(base: Reg, disp: i32) -> Mem {
    Mem {
        base,
        index: None,
        disp,
    }
}

pub const fn mem_idx(base: Reg, index: Reg, scale: u8, disp: i32) -> Mem {
    Mem {
        base,
        index: Some((index, scale)),
        disp,
    }
}

/// A place in the code, bound once.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Label(pub u32);

/// The sections of the final image that code can refer to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sect {
    Text,
    Rodata,
    Data,
    /// The import address table of a Windows image.
    Idata,
}

/// The virtual addresses of the sections of the final image.
#[derive(Clone, Copy, Debug, Default)]
pub struct SectAddrs {
    pub text: u64,
    pub rodata: u64,
    pub data: u64,
    pub idata: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Alu {
    Add = 0,
    Or = 1,
    And = 4,
    Sub = 5,
    Xor = 6,
    Cmp = 7,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shift {
    Shl = 4,
    Shr = 5,
    Sar = 7,
}

/// Condition codes, as the low four bits of `jcc`, `setcc` and `cmovcc`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cc {
    O = 0,
    No = 1,
    B = 2,
    Ae = 3,
    E = 4,
    Ne = 5,
    Be = 6,
    A = 7,
    S = 8,
    Ns = 9,
    P = 10,
    Np = 11,
    L = 12,
    Ge = 13,
    Le = 14,
    G = 15,
}

impl Cc {
    /// The condition that holds exactly when this one does not (they differ in the lowest bit).
    pub fn invert(self) -> Cc {
        match self {
            Cc::O => Cc::No,
            Cc::No => Cc::O,
            Cc::B => Cc::Ae,
            Cc::Ae => Cc::B,
            Cc::E => Cc::Ne,
            Cc::Ne => Cc::E,
            Cc::Be => Cc::A,
            Cc::A => Cc::Be,
            Cc::S => Cc::Ns,
            Cc::Ns => Cc::S,
            Cc::P => Cc::Np,
            Cc::Np => Cc::P,
            Cc::L => Cc::Ge,
            Cc::Ge => Cc::L,
            Cc::Le => Cc::G,
            Cc::G => Cc::Le,
        }
    }
}

/// Scalar floating-point arithmetic.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sse {
    Add = 0x58,
    Mul = 0x59,
    Sub = 0x5C,
    Div = 0x5E,
}

enum Rm {
    Reg(u8),
    Mem(Mem),
    /// `[rip + label]`
    Rip(Label),
    /// `[rip + section offset]`
    RipSect(Sect, u32),
}

enum FixKind {
    /// A 32-bit offset from the end of the field (plus `extra` bytes that follow it) to a label.
    Label(Label),
    Sect(Sect, u32),
}

struct Fixup {
    /// Where the 4-byte field is.
    at: usize,
    /// Bytes of instruction that come after the field (an immediate).
    extra: usize,
    kind: FixKind,
}

#[derive(Default)]
pub struct Asm {
    pub code: Vec<u8>,
    labels: Vec<Option<usize>>,
    fixups: Vec<Fixup>,
    /// Keep jumps (and the compare that fuses with one) clear of 32-byte boundaries: a jump that
    /// crosses or ends at one runs slowly on Skylake-derived Intel cores (the JCC erratum).
    pub pad_jumps: bool,
    /// The position of the latest label bound.
    last_bound: usize,
}

fn fits_i8(v: i64) -> bool {
    (-128..=127).contains(&v)
}

fn fits_i32(v: i64) -> bool {
    (i32::MIN as i64..=i32::MAX as i64).contains(&v)
}

impl Asm {
    pub fn new() -> Asm {
        Asm::default()
    }

    pub fn here(&self) -> usize {
        self.code.len()
    }

    pub fn new_label(&mut self) -> Label {
        self.labels.push(None);
        Label(self.labels.len() as u32 - 1)
    }

    /// Binds the label to the current position.
    pub fn bind(&mut self, l: Label) {
        debug_assert!(self.labels[l.0 as usize].is_none(), "label bound twice");
        self.labels[l.0 as usize] = Some(self.code.len());
        self.last_bound = self.code.len();
    }

    pub fn label_offset(&self, l: Label) -> Option<usize> {
        self.labels[l.0 as usize]
    }

    pub fn byte(&mut self, b: u8) {
        self.code.push(b);
    }

    /// `n` bytes of no-ops, as few instructions as possible.
    pub fn nops(&mut self, mut n: usize) {
        const NOPS: [&[u8]; 9] = [
            &[0x90],
            &[0x66, 0x90],
            &[0x0F, 0x1F, 0x00],
            &[0x0F, 0x1F, 0x40, 0x00],
            &[0x0F, 0x1F, 0x44, 0x00, 0x00],
            &[0x66, 0x0F, 0x1F, 0x44, 0x00, 0x00],
            &[0x0F, 0x1F, 0x80, 0x00, 0x00, 0x00, 0x00],
            &[0x0F, 0x1F, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00],
            &[0x66, 0x0F, 0x1F, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00],
        ];
        while n > 0 {
            let k = n.min(NOPS.len());
            self.code.extend_from_slice(NOPS[k - 1]);
            n -= k;
        }
    }

    /// Pads with no-ops up to a multiple of `to` (a power of two) when that takes at most `max_skip` bytes.
    pub fn align(&mut self, to: usize, max_skip: usize) {
        let skip = self.code.len().wrapping_neg() & (to - 1);
        if skip != 0 && skip <= max_skip {
            self.nops(skip);
        }
    }

    /// Called before emitting a jump of `len` bytes. `start` is where the instruction that sets its flags
    /// begins (the pair fuses into one micro-operation), or `here()` for a jump on its own. If the
    /// bytes from `start` to the end of the jump would cross a 32-byte boundary or end on one, no-ops
    /// move them to the next boundary. The padding goes before `start`, so the pair stays together.
    pub fn guard_jump(&mut self, start: usize, len: usize) {
        if !self.pad_jumps {
            return;
        }
        let end = self.code.len() + len;
        if end - start >= 32 || (start / 32 == (end - 1) / 32 && !end.is_multiple_of(32)) {
            return;
        }
        let pad = 32 - start % 32;
        if start == self.code.len() {
            self.nops(pad);
        } else if self.fixups.last().is_none_or(|f| f.at < start) && self.last_bound <= start {
            // Insert the no-ops in front of the instruction that sets the flags. A label that sits there
            // keeps pointing at the no-ops, which is harmless.
            let tail = self.code.split_off(start);
            self.nops(pad);
            self.code.extend_from_slice(&tail);
        } else {
            let here = self.code.len();
            self.guard_jump(here, len);
        }
    }

    pub fn bytes(&mut self, b: &[u8]) {
        self.code.extend_from_slice(b);
    }

    fn imm(&mut self, v: i64, n: usize) {
        self.code.extend_from_slice(&v.to_le_bytes()[..n]);
    }

    // ---- The generic encoder ----------------------------------------------------------------------

    /// `prefix? REX? opcode modrm sib? disp?`; `reg` is the register field (or an opcode extension).
    /// `imm_after` is the number of immediate bytes the caller appends, which matters for `rip` offsets.
    #[allow(clippy::too_many_arguments)]
    fn op(
        &mut self,
        prefix: Option<u8>,
        w: bool,
        opcode: &[u8],
        reg: u8,
        rm: Rm,
        force_rex: bool,
        imm_after: usize,
    ) {
        if let Some(p) = prefix {
            self.byte(p);
        }
        let b = match &rm {
            Rm::Reg(r) => *r >> 3,
            Rm::Mem(m) => m.base.0 >> 3,
            _ => 0,
        };
        let x = match &rm {
            Rm::Mem(Mem {
                index: Some((i, _)),
                ..
            }) => i.0 >> 3,
            _ => 0,
        };
        let rex = 0x40 | (u8::from(w) << 3) | ((reg >> 3) << 2) | (x << 1) | b;
        if rex != 0x40 || force_rex {
            self.byte(rex);
        }
        self.bytes(opcode);
        let r = (reg & 7) << 3;
        match rm {
            Rm::Reg(x) => self.byte(0xC0 | r | (x & 7)),
            Rm::Mem(m) => {
                let base = m.base.0 & 7;
                let (md, dlen) = if m.disp == 0 && base != 5 {
                    (0x00, 0)
                } else if fits_i8(m.disp as i64) {
                    (0x40, 1)
                } else {
                    (0x80, 4)
                };
                if let Some((i, scale)) = m.index {
                    assert!(i != RSP, "rsp cannot be an index");
                    let ss = match scale {
                        1 => 0,
                        2 => 1,
                        4 => 2,
                        _ => 3,
                    };
                    self.byte(md | r | 4);
                    self.byte(ss << 6 | (i.0 & 7) << 3 | base);
                } else if base == 4 {
                    self.byte(md | r | 4);
                    self.byte(0x24);
                } else {
                    self.byte(md | r | base);
                }
                if dlen > 0 {
                    self.imm(m.disp as i64, dlen);
                }
            }
            Rm::Rip(l) => {
                self.byte(r | 5);
                self.fixups.push(Fixup {
                    at: self.code.len(),
                    extra: imm_after,
                    kind: FixKind::Label(l),
                });
                self.imm(0, 4);
            }
            Rm::RipSect(s, off) => {
                self.byte(r | 5);
                self.fixups.push(Fixup {
                    at: self.code.len(),
                    extra: imm_after,
                    kind: FixKind::Sect(s, off),
                });
                self.imm(0, 4);
            }
        }
    }

    /// An integer instruction on `size` bytes (1, 2, 4 or 8) with the given opcode.
    fn int_op(
        &mut self,
        size: u8,
        opcode: &[u8],
        reg: u8,
        rm: Rm,
        byte_regs: &[Reg],
        imm_after: usize,
    ) {
        let prefix = (size == 2).then_some(0x66);
        let force = size == 1 && byte_regs.iter().any(|r| (4..8).contains(&r.0));
        self.op(prefix, size == 8, opcode, reg, rm, force, imm_after);
    }

    // ---- Moves --------------------------------------------------------------------------------------

    pub fn mov_rr(&mut self, dst: Reg, src: Reg, size: u8) {
        let op = if size == 1 { 0x88 } else { 0x89 };
        self.int_op(size, &[op], src.0, Rm::Reg(dst.0), &[dst, src], 0);
    }

    /// Loads a constant into the whole register, in the shortest form.
    pub fn mov_ri(&mut self, dst: Reg, v: i64) {
        if (0..=u32::MAX as i64).contains(&v) {
            // `mov r32, imm32` clears the upper half.
            if dst.0 >= 8 {
                self.byte(0x41);
            }
            self.byte(0xB8 + (dst.0 & 7));
            self.imm(v, 4);
        } else if fits_i32(v) {
            self.op(None, true, &[0xC7], 0, Rm::Reg(dst.0), false, 4);
            self.imm(v, 4);
        } else {
            self.byte(0x48 | (dst.0 >> 3));
            self.byte(0xB8 + (dst.0 & 7));
            self.imm(v, 8);
        }
    }

    pub fn load(&mut self, dst: Reg, m: Mem, size: u8) {
        let op = if size == 1 { 0x8A } else { 0x8B };
        self.int_op(size, &[op], dst.0, Rm::Mem(m), &[dst], 0);
    }

    pub fn store(&mut self, m: Mem, src: Reg, size: u8) {
        let op = if size == 1 { 0x88 } else { 0x89 };
        self.int_op(size, &[op], src.0, Rm::Mem(m), &[src], 0);
    }

    /// Stores a sign-extended 32-bit constant.
    pub fn store_imm(&mut self, m: Mem, v: i32, size: u8) {
        if size == 1 {
            self.int_op(1, &[0xC6], 0, Rm::Mem(m), &[], 1);
            self.imm(v as i64, 1);
        } else {
            let n = if size == 2 { 2 } else { 4 };
            self.int_op(size, &[0xC7], 0, Rm::Mem(m), &[], n);
            self.imm(v as i64, n);
        }
    }

    /// Loads `size` bytes and widens to 32 or 64 bits, with zeros or the sign.
    pub fn load_ext(&mut self, dst: Reg, m: Mem, size: u8, signed: bool, to64: bool) {
        match (size, signed) {
            (1, false) => self.op(None, to64, &[0x0F, 0xB6], dst.0, Rm::Mem(m), false, 0),
            (2, false) => self.op(None, to64, &[0x0F, 0xB7], dst.0, Rm::Mem(m), false, 0),
            (1, true) => self.op(None, to64, &[0x0F, 0xBE], dst.0, Rm::Mem(m), false, 0),
            (2, true) => self.op(None, to64, &[0x0F, 0xBF], dst.0, Rm::Mem(m), false, 0),
            (4, true) => self.op(None, true, &[0x63], dst.0, Rm::Mem(m), false, 0),
            (4, false) => self.op(None, false, &[0x8B], dst.0, Rm::Mem(m), false, 0),
            _ => self.load(dst, m, 8),
        }
    }

    /// Widens the low `size` bytes of `src` into `dst` (32 or 64 bits).
    pub fn extend_rr(&mut self, dst: Reg, src: Reg, size: u8, signed: bool, to64: bool) {
        let force = size == 1 && (4..8).contains(&src.0);
        match (size, signed) {
            (1, false) => self.op(None, to64, &[0x0F, 0xB6], dst.0, Rm::Reg(src.0), force, 0),
            (2, false) => self.op(None, to64, &[0x0F, 0xB7], dst.0, Rm::Reg(src.0), false, 0),
            (1, true) => self.op(None, to64, &[0x0F, 0xBE], dst.0, Rm::Reg(src.0), force, 0),
            (2, true) => self.op(None, to64, &[0x0F, 0xBF], dst.0, Rm::Reg(src.0), false, 0),
            (4, true) => self.op(None, true, &[0x63], dst.0, Rm::Reg(src.0), false, 0),
            (4, false) => self.mov_rr(dst, src, 4),
            _ => self.mov_rr(dst, src, 8),
        }
    }

    pub fn lea(&mut self, dst: Reg, m: Mem) {
        self.op(None, true, &[0x8D], dst.0, Rm::Mem(m), false, 0);
    }

    /// `lea dst, [rip + label]`
    pub fn lea_label(&mut self, dst: Reg, l: Label) {
        self.op(None, true, &[0x8D], dst.0, Rm::Rip(l), false, 0);
    }

    /// `lea dst, [rip + section + offset]`
    pub fn lea_sect(&mut self, dst: Reg, s: Sect, off: u32) {
        self.op(None, true, &[0x8D], dst.0, Rm::RipSect(s, off), false, 0);
    }

    pub fn push(&mut self, r: Reg) {
        if r.0 >= 8 {
            self.byte(0x41);
        }
        self.byte(0x50 + (r.0 & 7));
    }

    pub fn pop(&mut self, r: Reg) {
        if r.0 >= 8 {
            self.byte(0x41);
        }
        self.byte(0x58 + (r.0 & 7));
    }

    // ---- Arithmetic ---------------------------------------------------------------------------------

    pub fn alu_rr(&mut self, op: Alu, dst: Reg, src: Reg, size: u8) {
        let o = (op as u8) << 3 | u8::from(size != 1);
        self.int_op(size, &[o], src.0, Rm::Reg(dst.0), &[dst, src], 0);
    }

    /// `dst = dst op [mem]`
    pub fn alu_rm(&mut self, op: Alu, dst: Reg, m: Mem, size: u8) {
        let o = (op as u8) << 3 | if size == 1 { 2 } else { 3 };
        self.int_op(size, &[o], dst.0, Rm::Mem(m), &[dst], 0);
    }

    pub fn alu_ri(&mut self, op: Alu, dst: Reg, v: i32, size: u8) {
        self.alu_ixm(op, Rm::Reg(dst.0), v, size, &[dst]);
    }

    /// `[mem] = [mem] op imm`
    pub fn alu_mi(&mut self, op: Alu, m: Mem, v: i32, size: u8) {
        self.alu_ixm(op, Rm::Mem(m), v, size, &[]);
    }

    fn alu_ixm(&mut self, op: Alu, rm: Rm, v: i32, size: u8, regs: &[Reg]) {
        let ext = op as u8;
        // The assembler's short forms for the accumulator.
        if matches!(rm, Rm::Reg(0)) && (size == 1 || !fits_i8(v as i64)) {
            let n = match size {
                1 => 1,
                2 => 2,
                _ => 4,
            };
            let o = (ext << 3) | if size == 1 { 4 } else { 5 };
            self.int_op(size, &[o], 0, Rm::Reg(0), &[], 0);
            // `int_op` writes a mod-rm byte; the accumulator forms have none.
            self.code.pop();
            self.imm(v as i64, n);
            return;
        }
        if size == 1 {
            self.int_op(1, &[0x80], ext, rm, regs, 1);
            self.imm(v as i64, 1);
        } else if fits_i8(v as i64) {
            self.int_op(size, &[0x83], ext, rm, regs, 1);
            self.imm(v as i64, 1);
        } else {
            let n = if size == 2 { 2 } else { 4 };
            self.int_op(size, &[0x81], ext, rm, regs, n);
            self.imm(v as i64, n);
        }
    }

    pub fn test_rr(&mut self, a: Reg, b: Reg, size: u8) {
        let op = if size == 1 { 0x84 } else { 0x85 };
        self.int_op(size, &[op], b.0, Rm::Reg(a.0), &[a, b], 0);
    }

    pub fn imul_rr(&mut self, dst: Reg, src: Reg, size: u8) {
        self.int_op(size, &[0x0F, 0xAF], dst.0, Rm::Reg(src.0), &[], 0);
    }

    /// `dst = src * imm` (the three-operand form; `size` is 4 or 8).
    pub fn imul_rri(&mut self, dst: Reg, src: Reg, v: i32, size: u8) {
        if fits_i8(v as i64) {
            self.int_op(size, &[0x6B], dst.0, Rm::Reg(src.0), &[], 1);
            self.imm(v as i64, 1);
        } else {
            self.int_op(size, &[0x69], dst.0, Rm::Reg(src.0), &[], 4);
            self.imm(v as i64, 4);
        }
    }

    pub fn neg(&mut self, r: Reg, size: u8) {
        let op = if size == 1 { 0xF6 } else { 0xF7 };
        self.int_op(size, &[op], 3, Rm::Reg(r.0), &[r], 0);
    }

    pub fn not(&mut self, r: Reg, size: u8) {
        let op = if size == 1 { 0xF6 } else { 0xF7 };
        self.int_op(size, &[op], 2, Rm::Reg(r.0), &[r], 0);
    }

    /// Unsigned multiply of `rax` by `r` into `rdx:rax` (sets the carry flag when `rdx` is not zero).
    pub fn mul(&mut self, r: Reg, size: u8) {
        let op = if size == 1 { 0xF6 } else { 0xF7 };
        self.int_op(size, &[op], 4, Rm::Reg(r.0), &[r], 0);
    }

    /// Signed `rdx:rax = rax * r`.
    pub fn imul1(&mut self, r: Reg, size: u8) {
        let op = if size == 1 { 0xF6 } else { 0xF7 };
        self.int_op(size, &[op], 5, Rm::Reg(r.0), &[r], 0);
    }

    /// Unsigned divide of `rdx:rax` by `r`.
    pub fn div(&mut self, r: Reg, size: u8) {
        let op = if size == 1 { 0xF6 } else { 0xF7 };
        self.int_op(size, &[op], 6, Rm::Reg(r.0), &[r], 0);
    }

    /// Signed divide of `rdx:rax` by `r`.
    pub fn idiv(&mut self, r: Reg, size: u8) {
        let op = if size == 1 { 0xF6 } else { 0xF7 };
        self.int_op(size, &[op], 7, Rm::Reg(r.0), &[r], 0);
    }

    /// Sign-extends `eax` into `edx` (size 4) or `rax` into `rdx` (size 8).
    pub fn sign_extend_acc(&mut self, size: u8) {
        if size == 8 {
            self.byte(0x48);
        }
        self.byte(0x99);
    }

    pub fn shift_cl(&mut self, op: Shift, r: Reg, size: u8) {
        let o = if size == 1 { 0xD2 } else { 0xD3 };
        self.int_op(size, &[o], op as u8, Rm::Reg(r.0), &[r], 0);
    }

    pub fn shift_imm(&mut self, op: Shift, r: Reg, n: u8, size: u8) {
        let o = if size == 1 { 0xC0 } else { 0xC1 };
        self.int_op(size, &[o], op as u8, Rm::Reg(r.0), &[r], 1);
        self.byte(n);
    }

    /// `set<cc> r8`, then the caller usually widens with `extend_rr`.
    pub fn setcc(&mut self, cc: Cc, r: Reg) {
        let force = (4..8).contains(&r.0);
        self.op(
            None,
            false,
            &[0x0F, 0x90 + cc as u8],
            0,
            Rm::Reg(r.0),
            force,
            0,
        );
    }

    pub fn cmov(&mut self, cc: Cc, dst: Reg, src: Reg, size: u8) {
        self.int_op(
            size,
            &[0x0F, 0x40 + cc as u8],
            dst.0,
            Rm::Reg(src.0),
            &[],
            0,
        );
    }

    // ---- Control flow -------------------------------------------------------------------------------

    fn rel32_to(&mut self, l: Label, extra: usize) {
        self.fixups.push(Fixup {
            at: self.code.len(),
            extra,
            kind: FixKind::Label(l),
        });
        self.imm(0, 4);
    }

    pub fn jmp(&mut self, l: Label) {
        self.guard_jump(self.code.len(), 5);
        self.byte(0xE9);
        self.rel32_to(l, 0);
    }

    pub fn jcc(&mut self, cc: Cc, l: Label) {
        self.guard_jump(self.code.len(), 6);
        self.bytes(&[0x0F, 0x80 + cc as u8]);
        self.rel32_to(l, 0);
    }

    pub fn call(&mut self, l: Label) {
        self.byte(0xE8);
        self.rel32_to(l, 0);
    }

    pub fn call_reg(&mut self, r: Reg) {
        self.op(None, false, &[0xFF], 2, Rm::Reg(r.0), false, 0);
    }

    /// `call [rip + section + offset]`: through a pointer stored in memory (a Windows import).
    pub fn call_mem_sect(&mut self, s: Sect, off: u32) {
        self.op(None, false, &[0xFF], 2, Rm::RipSect(s, off), false, 0);
    }

    pub fn jmp_reg(&mut self, r: Reg) {
        self.op(None, false, &[0xFF], 4, Rm::Reg(r.0), false, 0);
    }

    pub fn ret(&mut self) {
        self.byte(0xC3);
    }

    pub fn syscall(&mut self) {
        self.bytes(&[0x0F, 0x05]);
    }

    pub fn ud2(&mut self) {
        self.bytes(&[0x0F, 0x0B]);
    }

    /// Copies `rcx` bytes from `[rsi]` to `[rdi]`.
    pub fn rep_movsb(&mut self) {
        self.bytes(&[0xF3, 0xA4]);
    }

    /// Fills `rcx` bytes at `[rdi]` with `al`.
    pub fn rep_stosb(&mut self) {
        self.bytes(&[0xF3, 0xAA]);
    }

    /// Fills `rcx` elements of `size` bytes at `[rdi]` with `al`, `ax`, `eax` or `rax`.
    pub fn rep_stos(&mut self, size: u8) {
        match size {
            1 => self.bytes(&[0xF3, 0xAA]),
            2 => self.bytes(&[0x66, 0xF3, 0xAB]),
            4 => self.bytes(&[0xF3, 0xAB]),
            _ => self.bytes(&[0xF3, 0x48, 0xAB]),
        }
    }

    // ---- SSE ----------------------------------------------------------------------------------------

    fn sse(&mut self, prefix: Option<u8>, w: bool, opcode: &[u8], reg: u8, rm: Rm) {
        self.op(prefix, w, opcode, reg, rm, false, 0);
    }

    /// Loads a float (`double`: 8 bytes, else 4) from memory.
    pub fn fload(&mut self, dst: Xmm, m: Mem, double: bool) {
        let p = if double { 0xF2 } else { 0xF3 };
        self.sse(Some(p), false, &[0x0F, 0x10], dst.0, Rm::Mem(m));
    }

    /// 16 bytes, any alignment: for copying and clearing.
    pub fn movups_load(&mut self, dst: Xmm, m: Mem) {
        self.sse(None, false, &[0x0F, 0x10], dst.0, Rm::Mem(m));
    }

    pub fn movups_store(&mut self, m: Mem, src: Xmm) {
        self.sse(None, false, &[0x0F, 0x11], src.0, Rm::Mem(m));
    }

    pub fn fstore(&mut self, m: Mem, src: Xmm, double: bool) {
        let p = if double { 0xF2 } else { 0xF3 };
        self.sse(Some(p), false, &[0x0F, 0x11], src.0, Rm::Mem(m));
    }

    pub fn fmov_rr(&mut self, dst: Xmm, src: Xmm) {
        self.sse(None, false, &[0x0F, 0x28], dst.0, Rm::Reg(src.0));
    }

    pub fn farith(&mut self, op: Sse, dst: Xmm, src: Xmm, double: bool) {
        let p = if double { 0xF2 } else { 0xF3 };
        self.sse(Some(p), false, &[0x0F, op as u8], dst.0, Rm::Reg(src.0));
    }

    /// Compares and sets the flags like an unsigned compare; the parity flag marks "unordered".
    pub fn fcmp(&mut self, a: Xmm, b: Xmm, double: bool) {
        let p = double.then_some(0x66);
        self.sse(p, false, &[0x0F, 0x2E], a.0, Rm::Reg(b.0));
    }

    /// Integer (32 or 64 bits) to float.
    pub fn cvt_int_to_float(&mut self, dst: Xmm, src: Reg, src64: bool, double: bool) {
        let p = if double { 0xF2 } else { 0xF3 };
        self.sse(Some(p), src64, &[0x0F, 0x2A], dst.0, Rm::Reg(src.0));
    }

    /// Float to integer, truncating toward zero.
    pub fn cvt_float_to_int(&mut self, dst: Reg, src: Xmm, dst64: bool, double: bool) {
        let p = if double { 0xF2 } else { 0xF3 };
        self.sse(Some(p), dst64, &[0x0F, 0x2C], dst.0, Rm::Reg(src.0));
    }

    /// `to_double`: single to double, else double to single.
    pub fn cvt_float_resize(&mut self, dst: Xmm, src: Xmm, to_double: bool) {
        let p = if to_double { 0xF3 } else { 0xF2 };
        self.sse(Some(p), false, &[0x0F, 0x5A], dst.0, Rm::Reg(src.0));
    }

    pub fn fxor(&mut self, dst: Xmm, src: Xmm) {
        self.sse(None, false, &[0x0F, 0x57], dst.0, Rm::Reg(src.0));
    }

    /// Moves the raw bits of a general register into an SSE register.
    pub fn movq_to_xmm(&mut self, dst: Xmm, src: Reg, wide: bool) {
        self.sse(Some(0x66), wide, &[0x0F, 0x6E], dst.0, Rm::Reg(src.0));
    }

    /// Moves the raw bits of an SSE register into a general register.
    pub fn movq_from_xmm(&mut self, dst: Reg, src: Xmm, wide: bool) {
        self.sse(Some(0x66), wide, &[0x0F, 0x7E], src.0, Rm::Reg(dst.0));
    }

    // ---- x87 (only for the floating-point remainder) ---------------------------------------------------

    /// `fld` of a 4- or 8-byte float onto the x87 stack.
    pub fn x87_load(&mut self, m: Mem, double: bool) {
        let op = if double { 0xDD } else { 0xD9 };
        self.op(None, false, &[op], 0, Rm::Mem(m), false, 0);
    }

    /// `fstp`: store the top of the x87 stack and pop it.
    pub fn x87_store_pop(&mut self, m: Mem, double: bool) {
        let op = if double { 0xDD } else { 0xD9 };
        self.op(None, false, &[op], 3, Rm::Mem(m), false, 0);
    }

    /// `fprem`: partial remainder of `st0 / st1`; repeat while the C2 flag of the status word is set.
    pub fn x87_prem(&mut self) {
        self.bytes(&[0xD9, 0xF8]);
    }

    /// `fnstsw ax`
    pub fn x87_status_ax(&mut self) {
        self.bytes(&[0xDF, 0xE0]);
    }

    /// `fstp st(1)`: drop the second entry of the x87 stack.
    pub fn x87_drop_second(&mut self) {
        self.bytes(&[0xDD, 0xD9]);
    }

    // ---- Finishing ----------------------------------------------------------------------------------

    /// Resolves every reference. `text`, `rodata` and `data` are the virtual addresses the sections
    /// will have; the code itself starts at `text`.
    pub fn finish(&self, addrs: SectAddrs) -> Result<Vec<u8>, String> {
        let SectAddrs {
            text,
            rodata,
            data,
            idata,
        } = addrs;
        let mut out = self.code.clone();
        for f in &self.fixups {
            let end = text as i64 + f.at as i64 + 4 + f.extra as i64;
            let target = match f.kind {
                FixKind::Label(l) => {
                    let off = self.labels[l.0 as usize]
                        .ok_or_else(|| format!("label {} was never bound", l.0))?;
                    text as i64 + off as i64
                }
                FixKind::Sect(s, off) => {
                    let base = match s {
                        Sect::Text => text,
                        Sect::Rodata => rodata,
                        Sect::Data => data,
                        Sect::Idata => idata,
                    };
                    base as i64 + off as i64
                }
            };
            let rel = target - end;
            if !fits_i32(rel) {
                return Err("a reference is too far away".to_string());
            }
            out[f.at..f.at + 4].copy_from_slice(&(rel as i32).to_le_bytes());
        }
        Ok(out)
    }
}

#[cfg(test)]
mod pad_tests {
    use super::*;

    #[test]
    fn a_jump_never_crosses_or_ends_at_a_32_byte_boundary() {
        for lead in 0..70 {
            let mut a = Asm::new();
            a.pad_jumps = true;
            let l = a.new_label();
            a.bind(l);
            a.nops(lead);
            let start = a.here();
            a.alu_rr(Alu::Cmp, RAX, RCX, 8);
            a.guard_jump(start, 6);
            let jstart = a.here();
            a.jcc(Cc::E, l);
            let (s, e) = (a.code.len() - 6, a.code.len());
            assert!(jstart == s);
            assert_eq!(s / 32, (e - 1) / 32, "crosses with {lead} lead bytes");
            assert_ne!(e % 32, 0, "ends on a boundary with {lead} lead bytes");
            // the compare stays right in front of the jump
            assert_eq!(&a.code[s - 3..s], &[0x48, 0x39, 0xC8]);
        }
    }

    #[test]
    fn align_pads_only_when_cheap() {
        let mut a = Asm::new();
        a.nops(3);
        a.align(16, 8);
        assert_eq!(a.here(), 3);
        a.nops(6);
        a.align(16, 8);
        assert_eq!(a.here(), 16);
    }
}
