//! An AArch64 instruction encoder: only the forms the code generator uses (card A-10, stage R1).
//!
//! Every instruction is one 32-bit word. `Asm` collects the words and labels; branches to labels and
//! references to the data sections are patched when the final layout is known ([`Asm::finish`]).
//! Every encoding is checked against the system assembler by the tests (golden words, plus a live
//! check when `aarch64-linux-gnu-as` is installed).
//!
//! Register conventions of the generator (not enforced here): `x16` and `x17` (the "intra-procedure
//! call" registers) are the scratch registers of this file's helpers, `x29` is the frame pointer.

pub use super::x86::{Sect, SectAddrs};

/// A general-purpose register. Number 31 is the stack pointer or the zero register, depending on
/// the instruction (the methods below say which).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Reg(pub u8);

pub const X0: Reg = Reg(0);
pub const X1: Reg = Reg(1);
pub const X2: Reg = Reg(2);
pub const X3: Reg = Reg(3);
pub const X4: Reg = Reg(4);
pub const X5: Reg = Reg(5);
pub const X6: Reg = Reg(6);
pub const X7: Reg = Reg(7);
pub const X8: Reg = Reg(8);
pub const X9: Reg = Reg(9);
pub const X10: Reg = Reg(10);
pub const X11: Reg = Reg(11);
pub const X12: Reg = Reg(12);
pub const X13: Reg = Reg(13);
pub const X14: Reg = Reg(14);
pub const X15: Reg = Reg(15);
pub const X16: Reg = Reg(16);
pub const X17: Reg = Reg(17);
pub const X19: Reg = Reg(19);
pub const X20: Reg = Reg(20);
pub const X21: Reg = Reg(21);
pub const X22: Reg = Reg(22);
pub const X23: Reg = Reg(23);
pub const X24: Reg = Reg(24);
pub const X25: Reg = Reg(25);
pub const X26: Reg = Reg(26);
pub const X27: Reg = Reg(27);
pub const X28: Reg = Reg(28);
pub const FP: Reg = Reg(29);
pub const LR: Reg = Reg(30);
pub const SP: Reg = Reg(31);
pub const XZR: Reg = Reg(31);

/// A floating-point / vector register (`v0` to `v31`, seen as `s`, `d` or `q`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct V(pub u8);

/// A condition code.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cond {
    Eq = 0,
    Ne = 1,
    /// Carry set (unsigned `>=`).
    Hs = 2,
    /// Carry clear (unsigned `<`).
    Lo = 3,
    Mi = 4,
    Pl = 5,
    Vs = 6,
    Vc = 7,
    Hi = 8,
    Ls = 9,
    Ge = 10,
    Lt = 11,
    Gt = 12,
    Le = 13,
}

impl Cond {
    /// The opposite condition (the codes come in pairs that differ in the lowest bit).
    pub fn invert(self) -> Cond {
        use Cond::*;
        match self {
            Eq => Ne,
            Ne => Eq,
            Hs => Lo,
            Lo => Hs,
            Mi => Pl,
            Pl => Mi,
            Vs => Vc,
            Vc => Vs,
            Hi => Ls,
            Ls => Hi,
            Ge => Lt,
            Lt => Ge,
            Gt => Le,
            Le => Gt,
        }
    }
}

/// How a shifted register operand is shifted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shift {
    Lsl = 0,
    Lsr = 1,
    Asr = 2,
}

/// A place in the code, bound once.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Label(pub u32);

#[derive(Clone, Copy, Debug)]
enum Target {
    Label(Label),
    Sect(Sect, u32),
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    /// `b` and `bl`: 26 bits of word offset.
    Branch26,
    /// `b.cond`, `cbz`, `cbnz`: 19 bits of word offset.
    Branch19,
    /// `adrp`: the page of the target.
    Page,
    /// `add rd, rn, #imm12`: the low 12 bits of the target.
    Lo12,
}

#[derive(Clone, Copy, Debug)]
struct Fixup {
    at: usize,
    kind: Kind,
    target: Target,
}

#[derive(Default)]
pub struct Asm {
    pub code: Vec<u8>,
    labels: Vec<Option<usize>>,
    fixups: Vec<Fixup>,
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
    }

    pub fn label_offset(&self, l: Label) -> Option<usize> {
        self.labels[l.0 as usize]
    }

    /// Emits one instruction word.
    pub fn word(&mut self, w: u32) {
        self.code.extend_from_slice(&w.to_le_bytes());
    }

    /// Pads with `nop`s to a multiple of `to` bytes (a power of two, at least 4).
    pub fn align(&mut self, to: usize) {
        while !self.code.len().is_multiple_of(to) {
            self.nop();
        }
    }

    fn fix(&mut self, kind: Kind, target: Target) {
        self.fixups.push(Fixup {
            at: self.code.len(),
            kind,
            target,
        });
    }

    // ---- Moves -----------------------------------------------------------------------------------

    fn mov_wide(&mut self, base: u32, rd: Reg, imm16: u32, hw: u32) {
        self.word(base | hw << 21 | imm16 << 5 | rd.0 as u32);
    }

    /// Loads any 64-bit constant: `movz`/`movn` and then `movk` for each further 16-bit piece that
    /// differs from the filler.
    pub fn mov_ri(&mut self, rd: Reg, v: i64) {
        let u = v as u64;
        let pieces = |x: u64| (0..4).filter(|i| (x >> (16 * i)) & 0xFFFF != 0).count();
        if pieces(!u) < pieces(u) {
            // Start from all ones: `movn`, then `movk` for the pieces that are not 0xFFFF.
            let n = !u;
            let first = (0..4).find(|i| (n >> (16 * i)) & 0xFFFF != 0).unwrap_or(0);
            self.mov_wide(
                0x9280_0000,
                rd,
                ((n >> (16 * first)) & 0xFFFF) as u32,
                first,
            );
            for i in 0..4 {
                let piece = (u >> (16 * i)) & 0xFFFF;
                if i != first && piece != 0xFFFF {
                    self.mov_wide(0xF280_0000, rd, piece as u32, i);
                }
            }
        } else {
            let first = (0..4).find(|i| (u >> (16 * i)) & 0xFFFF != 0).unwrap_or(0);
            self.mov_wide(
                0xD280_0000,
                rd,
                ((u >> (16 * first)) & 0xFFFF) as u32,
                first,
            );
            for i in 0..4 {
                let piece = (u >> (16 * i)) & 0xFFFF;
                if i != first && piece != 0 {
                    self.mov_wide(0xF280_0000, rd, piece as u32, i);
                }
            }
        }
    }

    pub fn movz(&mut self, rd: Reg, imm16: u16, hw: u32) {
        self.mov_wide(0xD280_0000, rd, imm16 as u32, hw);
    }

    pub fn movk(&mut self, rd: Reg, imm16: u16, hw: u32) {
        self.mov_wide(0xF280_0000, rd, imm16 as u32, hw);
    }

    /// `mov rd, rm` (not for the stack pointer).
    pub fn mov_rr(&mut self, rd: Reg, rm: Reg) {
        self.word(0xAA00_03E0 | (rm.0 as u32) << 16 | rd.0 as u32);
    }

    /// `mov wd, wm`: the low half, zero-extended.
    pub fn mov_w(&mut self, rd: Reg, rm: Reg) {
        self.word(0x2A00_03E0 | (rm.0 as u32) << 16 | rd.0 as u32);
    }

    /// `mov rd, sp` and `mov sp, rn`.
    pub fn mov_from_sp(&mut self, rd: Reg) {
        self.add_ri(rd, SP, 0);
    }

    pub fn mov_to_sp(&mut self, rn: Reg) {
        self.add_ri(SP, rn, 0);
    }

    // ---- Arithmetic ------------------------------------------------------------------------------

    fn addsub_imm(&mut self, base: u32, rd: Reg, rn: Reg, imm: u32) {
        if imm < 4096 {
            self.word(base | imm << 10 | (rn.0 as u32) << 5 | rd.0 as u32);
        } else {
            assert!(imm & 0xFFF == 0 && imm >> 12 < 4096, "immediate {imm}");
            self.word(base | 1 << 22 | (imm >> 12) << 10 | (rn.0 as u32) << 5 | rd.0 as u32);
        }
    }

    /// `add rd, rn, #imm` (`rd` and `rn` may be `sp`); `imm` is below 4096, or a multiple of 4096
    /// below 2^24.
    pub fn add_ri(&mut self, rd: Reg, rn: Reg, imm: u32) {
        self.addsub_imm(0x9100_0000, rd, rn, imm);
    }

    pub fn sub_ri(&mut self, rd: Reg, rn: Reg, imm: u32) {
        self.addsub_imm(0xD100_0000, rd, rn, imm);
    }

    pub fn adds_ri(&mut self, rd: Reg, rn: Reg, imm: u32) {
        self.addsub_imm(0xB100_0000, rd, rn, imm);
    }

    pub fn subs_ri(&mut self, rd: Reg, rn: Reg, imm: u32) {
        self.addsub_imm(0xF100_0000, rd, rn, imm);
    }

    pub fn cmp_ri(&mut self, rn: Reg, imm: u32) {
        self.subs_ri(XZR, rn, imm);
    }

    /// Whether `add`/`sub` can take this constant as an immediate (alone, with the sign flipped
    /// for the opposite instruction).
    pub fn fits_addsub(imm: u64) -> bool {
        imm < 4096 || (imm & 0xFFF == 0 && imm >> 12 < 4096)
    }

    /// `rd = rn + v` for any constant, using `x17` when it does not fit an immediate.
    pub fn add_imm(&mut self, rd: Reg, rn: Reg, v: i64) {
        if Self::fits_addsub(v.unsigned_abs()) {
            if v >= 0 {
                self.add_ri(rd, rn, v as u32);
            } else {
                self.sub_ri(rd, rn, v.unsigned_abs() as u32);
            }
        } else if v.unsigned_abs() < 1 << 24 {
            // Two instructions: the high 12 bits, then the low ones.
            let (hi, lo) = (v.unsigned_abs() & !0xFFF, v.unsigned_abs() & 0xFFF);
            if v >= 0 {
                self.add_ri(rd, rn, hi as u32);
                self.add_ri(rd, rd, lo as u32);
            } else {
                self.sub_ri(rd, rn, hi as u32);
                self.sub_ri(rd, rd, lo as u32);
            }
        } else {
            self.mov_ri(X17, v);
            self.add_ext(rd, rn, X17);
        }
    }

    /// `add rd, rn, rm, uxtx`: the form that takes `sp` as the base.
    pub fn add_ext(&mut self, rd: Reg, rn: Reg, rm: Reg) {
        self.word(0x8B20_6000 | (rm.0 as u32) << 16 | (rn.0 as u32) << 5 | rd.0 as u32);
    }

    fn shifted(&mut self, base: u32, rd: Reg, rn: Reg, rm: Reg, sh: Shift, amount: u32) {
        self.word(
            base | (sh as u32) << 22
                | (rm.0 as u32) << 16
                | amount << 10
                | (rn.0 as u32) << 5
                | rd.0 as u32,
        );
    }

    pub fn add_rr(&mut self, rd: Reg, rn: Reg, rm: Reg) {
        self.shifted(0x8B00_0000, rd, rn, rm, Shift::Lsl, 0);
    }

    pub fn sub_rr(&mut self, rd: Reg, rn: Reg, rm: Reg) {
        self.shifted(0xCB00_0000, rd, rn, rm, Shift::Lsl, 0);
    }

    pub fn adds_rr(&mut self, rd: Reg, rn: Reg, rm: Reg) {
        self.shifted(0xAB00_0000, rd, rn, rm, Shift::Lsl, 0);
    }

    pub fn subs_rr(&mut self, rd: Reg, rn: Reg, rm: Reg) {
        self.shifted(0xEB00_0000, rd, rn, rm, Shift::Lsl, 0);
    }

    /// `add rd, rn, rm, lsl #amount` and the like.
    pub fn add_rr_sh(&mut self, rd: Reg, rn: Reg, rm: Reg, sh: Shift, amount: u32) {
        self.shifted(0x8B00_0000, rd, rn, rm, sh, amount);
    }

    /// `cmp rn, rm, asr #amount` (the overflow test of a 64-bit signed product).
    pub fn cmp_rr_sh(&mut self, rn: Reg, rm: Reg, sh: Shift, amount: u32) {
        self.shifted(0xEB00_0000, XZR, rn, rm, sh, amount);
    }

    pub fn cmp_rr(&mut self, rn: Reg, rm: Reg) {
        self.subs_rr(XZR, rn, rm);
    }

    pub fn neg(&mut self, rd: Reg, rm: Reg) {
        self.sub_rr(rd, XZR, rm);
    }

    pub fn and_rr(&mut self, rd: Reg, rn: Reg, rm: Reg) {
        self.shifted(0x8A00_0000, rd, rn, rm, Shift::Lsl, 0);
    }

    pub fn orr_rr(&mut self, rd: Reg, rn: Reg, rm: Reg) {
        self.shifted(0xAA00_0000, rd, rn, rm, Shift::Lsl, 0);
    }

    pub fn eor_rr(&mut self, rd: Reg, rn: Reg, rm: Reg) {
        self.shifted(0xCA00_0000, rd, rn, rm, Shift::Lsl, 0);
    }

    pub fn tst_rr(&mut self, rn: Reg, rm: Reg) {
        self.shifted(0xEA00_0000, XZR, rn, rm, Shift::Lsl, 0);
    }

    pub fn mvn(&mut self, rd: Reg, rm: Reg) {
        self.shifted(0xAA20_0000, rd, XZR, rm, Shift::Lsl, 0);
    }

    /// The `N:immr:imms` field of a logical immediate, when `v` is one (a rotated run of ones,
    /// repeated at a power-of-two period).
    pub fn bitmask(v: u64) -> Option<u32> {
        if v == 0 || v == !0 {
            return None;
        }
        let mut size = 64u32;
        while size > 2 {
            let half = size / 2;
            let mask = (1u64 << half) - 1;
            if v & mask != (v >> half) & mask {
                break;
            }
            size = half;
        }
        let mask = if size == 64 { !0 } else { (1u64 << size) - 1 };
        let e = v & mask;
        let ones = e.count_ones();
        let run = if ones == 64 { !0 } else { (1u64 << ones) - 1 };
        let ror = |x: u64, r: u32| -> u64 {
            if r == 0 {
                x
            } else {
                ((x >> r) | (x << (size - r))) & mask
            }
        };
        let immr = (0..size).find(|&r| ror(run, r) == e)?;
        let imms = (0x3F & !(2 * size - 1)) | (ones - 1);
        let n = u32::from(size == 64);
        Some(n << 12 | immr << 6 | imms)
    }

    fn logical_imm(&mut self, base: u32, rd: Reg, rn: Reg, v: u64) -> bool {
        match Self::bitmask(v) {
            Some(f) => {
                self.word(base | f << 10 | (rn.0 as u32) << 5 | rd.0 as u32);
                true
            }
            None => false,
        }
    }

    /// `and rd, rn, #v`; false (nothing emitted) when `v` is not encodable.
    pub fn and_ri(&mut self, rd: Reg, rn: Reg, v: u64) -> bool {
        self.logical_imm(0x9200_0000, rd, rn, v)
    }

    pub fn orr_ri(&mut self, rd: Reg, rn: Reg, v: u64) -> bool {
        self.logical_imm(0xB200_0000, rd, rn, v)
    }

    pub fn eor_ri(&mut self, rd: Reg, rn: Reg, v: u64) -> bool {
        self.logical_imm(0xD200_0000, rd, rn, v)
    }

    pub fn tst_ri(&mut self, rn: Reg, v: u64) -> bool {
        self.logical_imm(0xF200_0000, XZR, rn, v)
    }

    pub fn mul(&mut self, rd: Reg, rn: Reg, rm: Reg) {
        self.madd(rd, rn, rm, XZR);
    }

    /// `rd = ra + rn * rm`
    pub fn madd(&mut self, rd: Reg, rn: Reg, rm: Reg, ra: Reg) {
        self.word(
            0x9B00_0000
                | (rm.0 as u32) << 16
                | (ra.0 as u32) << 10
                | (rn.0 as u32) << 5
                | rd.0 as u32,
        );
    }

    /// `rd = ra - rn * rm`
    pub fn msub(&mut self, rd: Reg, rn: Reg, rm: Reg, ra: Reg) {
        self.word(
            0x9B00_8000
                | (rm.0 as u32) << 16
                | (ra.0 as u32) << 10
                | (rn.0 as u32) << 5
                | rd.0 as u32,
        );
    }

    fn three(&mut self, base: u32, rd: Reg, rn: Reg, rm: Reg) {
        self.word(base | (rm.0 as u32) << 16 | (rn.0 as u32) << 5 | rd.0 as u32);
    }

    pub fn smulh(&mut self, rd: Reg, rn: Reg, rm: Reg) {
        self.three(0x9B40_7C00, rd, rn, rm);
    }

    pub fn umulh(&mut self, rd: Reg, rn: Reg, rm: Reg) {
        self.three(0x9BC0_7C00, rd, rn, rm);
    }

    pub fn sdiv(&mut self, rd: Reg, rn: Reg, rm: Reg) {
        self.three(0x9AC0_0C00, rd, rn, rm);
    }

    pub fn udiv(&mut self, rd: Reg, rn: Reg, rm: Reg) {
        self.three(0x9AC0_0800, rd, rn, rm);
    }

    pub fn lslv(&mut self, rd: Reg, rn: Reg, rm: Reg) {
        self.three(0x9AC0_2000, rd, rn, rm);
    }

    pub fn lsrv(&mut self, rd: Reg, rn: Reg, rm: Reg) {
        self.three(0x9AC0_2400, rd, rn, rm);
    }

    pub fn asrv(&mut self, rd: Reg, rn: Reg, rm: Reg) {
        self.three(0x9AC0_2800, rd, rn, rm);
    }

    fn bitfield(&mut self, base: u32, rd: Reg, rn: Reg, immr: u32, imms: u32) {
        self.word(base | immr << 16 | imms << 10 | (rn.0 as u32) << 5 | rd.0 as u32);
    }

    pub fn lsl_ri(&mut self, rd: Reg, rn: Reg, n: u32) {
        self.bitfield(0xD340_0000, rd, rn, (64 - n) & 63, 63 - n);
    }

    pub fn lsr_ri(&mut self, rd: Reg, rn: Reg, n: u32) {
        self.bitfield(0xD340_0000, rd, rn, n, 63);
    }

    pub fn asr_ri(&mut self, rd: Reg, rn: Reg, n: u32) {
        self.bitfield(0x9340_0000, rd, rn, n, 63);
    }

    /// `ubfx rd, rn, #lsb, #width`
    pub fn ubfx(&mut self, rd: Reg, rn: Reg, lsb: u32, width: u32) {
        self.bitfield(0xD340_0000, rd, rn, lsb, lsb + width - 1);
    }

    /// The low 8, 16 or 32 bits, zero-extended (`uxtb`, `uxth`, `mov w`).
    pub fn uxt(&mut self, rd: Reg, rn: Reg, bits: u32) {
        match bits {
            8 => self.bitfield(0x5300_0000, rd, rn, 0, 7),
            16 => self.bitfield(0x5300_0000, rd, rn, 0, 15),
            32 => self.mov_w(rd, rn),
            _ => unreachable!("uxt {bits}"),
        }
    }

    /// The low 8, 16 or 32 bits, sign-extended to 64 (`sxtb`, `sxth`, `sxtw`).
    pub fn sxt(&mut self, rd: Reg, rn: Reg, bits: u32) {
        self.bitfield(0x9340_0000, rd, rn, 0, bits - 1);
    }

    /// Count of leading zero bits of a 64-bit register.
    pub fn clz(&mut self, rd: Reg, rn: Reg) {
        self.word(0xDAC0_1000 | (rn.0 as u32) << 5 | rd.0 as u32);
    }

    pub fn cset(&mut self, rd: Reg, cc: Cond) {
        self.word(0x1A9F_07E0 | ((cc.invert() as u32) << 12) | rd.0 as u32);
    }

    /// `rd = cc ? rn : rm`
    pub fn csel(&mut self, rd: Reg, rn: Reg, rm: Reg, cc: Cond) {
        self.word(
            0x9A80_0000
                | (rm.0 as u32) << 16
                | (cc as u32) << 12
                | (rn.0 as u32) << 5
                | rd.0 as u32,
        );
    }

    // ---- Branches --------------------------------------------------------------------------------

    pub fn b(&mut self, l: Label) {
        self.fix(Kind::Branch26, Target::Label(l));
        self.word(0x1400_0000);
    }

    pub fn bl(&mut self, l: Label) {
        self.fix(Kind::Branch26, Target::Label(l));
        self.word(0x9400_0000);
    }

    pub fn b_cond(&mut self, cc: Cond, l: Label) {
        self.fix(Kind::Branch19, Target::Label(l));
        self.word(0x5400_0000 | cc as u32);
    }

    pub fn cbz(&mut self, rt: Reg, l: Label) {
        self.fix(Kind::Branch19, Target::Label(l));
        self.word(0xB400_0000 | rt.0 as u32);
    }

    pub fn cbnz(&mut self, rt: Reg, l: Label) {
        self.fix(Kind::Branch19, Target::Label(l));
        self.word(0xB500_0000 | rt.0 as u32);
    }

    pub fn br(&mut self, rn: Reg) {
        self.word(0xD61F_0000 | (rn.0 as u32) << 5);
    }

    pub fn blr(&mut self, rn: Reg) {
        self.word(0xD63F_0000 | (rn.0 as u32) << 5);
    }

    pub fn ret(&mut self) {
        self.word(0xD65F_03C0);
    }

    pub fn svc(&mut self) {
        self.word(0xD400_0001);
    }

    /// A breakpoint: the instruction code that must never run.
    pub fn brk(&mut self) {
        self.word(0xD420_0000);
    }

    pub fn nop(&mut self) {
        self.word(0xD503_201F);
    }

    /// `rd = address of section + offset` (`adrp` and `add`).
    pub fn lea_sect(&mut self, rd: Reg, s: Sect, off: u32) {
        self.fix(Kind::Page, Target::Sect(s, off));
        self.word(0x9000_0000 | rd.0 as u32);
        self.fix(Kind::Lo12, Target::Sect(s, off));
        self.word(0x9100_0000 | (rd.0 as u32) << 5 | rd.0 as u32);
    }

    /// `rd = address of a label` (`adrp` and `add`).
    pub fn lea_label(&mut self, rd: Reg, l: Label) {
        self.fix(Kind::Page, Target::Label(l));
        self.word(0x9000_0000 | rd.0 as u32);
        self.fix(Kind::Lo12, Target::Label(l));
        self.word(0x9100_0000 | (rd.0 as u32) << 5 | rd.0 as u32);
    }

    // ---- Memory ----------------------------------------------------------------------------------

    fn mem_op(&mut self, size_log: u32, opc: u32, vec: bool, rt: u8, base: Reg, off: i32) {
        let v = if vec { 0x0400_0000 } else { 0 };
        let scale = 1i32 << size_log;
        // A 128-bit register has size 0 and the top bit of `opc` set.
        let (size_bits, opc_bits) = if vec && size_log == 4 {
            (0, opc | 2)
        } else {
            (size_log, opc)
        };
        if off >= 0 && off % scale == 0 && off / scale < 4096 {
            self.word(
                size_bits << 30
                    | 0x3900_0000
                    | v
                    | opc_bits << 22
                    | ((off / scale) as u32) << 10
                    | (base.0 as u32) << 5
                    | rt as u32,
            );
        } else if (-256..256).contains(&off) {
            self.word(
                size_bits << 30
                    | 0x3800_0000
                    | v
                    | opc_bits << 22
                    | ((off as u32) & 0x1FF) << 12
                    | (base.0 as u32) << 5
                    | rt as u32,
            );
        } else {
            // Out of reach of both forms: the address goes through `x16`.
            self.mov_ri(X16, off as i64);
            self.add_ext(X16, base, X16);
            self.mem_op(size_log, opc, vec, rt, X16, 0);
        }
    }

    /// `ldr rt, [base, #off]` of `size` bytes (1, 2, 4 or 8), zero-extended.
    pub fn ldr(&mut self, rt: Reg, base: Reg, off: i32, size: u8) {
        self.mem_op(size.trailing_zeros(), 1, false, rt.0, base, off);
    }

    /// `ldrs[bhw] rt, [base, #off]`: sign-extended to 64 bits.
    pub fn ldrs(&mut self, rt: Reg, base: Reg, off: i32, size: u8) {
        self.mem_op(size.trailing_zeros(), 2, false, rt.0, base, off);
    }

    pub fn str(&mut self, rt: Reg, base: Reg, off: i32, size: u8) {
        self.mem_op(size.trailing_zeros(), 0, false, rt.0, base, off);
    }

    /// Loads a float or vector register: `size` is 4 (`s`), 8 (`d`) or 16 (`q`).
    pub fn ldr_f(&mut self, vt: V, base: Reg, off: i32, size: u8) {
        self.mem_op(size.trailing_zeros(), 1, true, vt.0, base, off);
    }

    pub fn str_f(&mut self, vt: V, base: Reg, off: i32, size: u8) {
        self.mem_op(size.trailing_zeros(), 0, true, vt.0, base, off);
    }

    /// `ldr rt, [base, index, lsl #shift]` (`shift` is 0 or the size's log2).
    pub fn ldr_idx(&mut self, rt: Reg, base: Reg, index: Reg, size: u8, scaled: bool) {
        self.idx_op(1, rt, base, index, size, scaled);
    }

    pub fn str_idx(&mut self, rt: Reg, base: Reg, index: Reg, size: u8, scaled: bool) {
        self.idx_op(0, rt, base, index, size, scaled);
    }

    fn idx_op(&mut self, opc: u32, rt: Reg, base: Reg, index: Reg, size: u8, scaled: bool) {
        self.word(
            size.trailing_zeros() << 30
                | 0x3820_6800
                | opc << 22
                | (index.0 as u32) << 16
                | u32::from(scaled) << 12
                | (base.0 as u32) << 5
                | rt.0 as u32,
        );
    }

    /// `stp rt, rt2, [base, #off]!` (pre-indexed): the usual way to push a pair.
    pub fn stp_pre(&mut self, rt: Reg, rt2: Reg, base: Reg, off: i32) {
        self.pair(0xA980_0000, rt, rt2, base, off);
    }

    /// `ldp rt, rt2, [base], #off` (post-indexed): the usual way to pop a pair.
    pub fn ldp_post(&mut self, rt: Reg, rt2: Reg, base: Reg, off: i32) {
        self.pair(0xA8C0_0000, rt, rt2, base, off);
    }

    /// `stp rt, rt2, [base, #off]` and `ldp`: `off` is a multiple of 8 in -512..504.
    pub fn stp(&mut self, rt: Reg, rt2: Reg, base: Reg, off: i32) {
        self.pair(0xA900_0000, rt, rt2, base, off);
    }

    pub fn ldp(&mut self, rt: Reg, rt2: Reg, base: Reg, off: i32) {
        self.pair(0xA940_0000, rt, rt2, base, off);
    }

    fn pair(&mut self, base_w: u32, rt: Reg, rt2: Reg, base: Reg, off: i32) {
        assert!(
            off % 8 == 0 && (-512..512).contains(&off),
            "pair offset {off}"
        );
        self.word(
            base_w
                | (((off / 8) as u32) & 0x7F) << 15
                | (rt2.0 as u32) << 10
                | (base.0 as u32) << 5
                | rt.0 as u32,
        );
    }

    // ---- Floating point ----------------------------------------------------------------------------

    fn fp2(&mut self, base: u32, double: bool, vd: V, vn: V) {
        let t = if double { 0x0040_0000 } else { 0 };
        self.word(base | t | (vn.0 as u32) << 5 | vd.0 as u32);
    }

    fn fp3(&mut self, base: u32, double: bool, vd: V, vn: V, vm: V) {
        let t = if double { 0x0040_0000 } else { 0 };
        self.word(base | t | (vm.0 as u32) << 16 | (vn.0 as u32) << 5 | vd.0 as u32);
    }

    pub fn fadd(&mut self, d: bool, vd: V, vn: V, vm: V) {
        self.fp3(0x1E20_2800, d, vd, vn, vm);
    }

    pub fn fsub(&mut self, d: bool, vd: V, vn: V, vm: V) {
        self.fp3(0x1E20_3800, d, vd, vn, vm);
    }

    pub fn fmul(&mut self, d: bool, vd: V, vn: V, vm: V) {
        self.fp3(0x1E20_0800, d, vd, vn, vm);
    }

    pub fn fdiv(&mut self, d: bool, vd: V, vn: V, vm: V) {
        self.fp3(0x1E20_1800, d, vd, vn, vm);
    }

    pub fn fneg(&mut self, d: bool, vd: V, vn: V) {
        self.fp2(0x1E21_4000, d, vd, vn);
    }

    pub fn fmov_rr(&mut self, d: bool, vd: V, vn: V) {
        self.fp2(0x1E20_4000, d, vd, vn);
    }

    pub fn fcmp(&mut self, d: bool, vn: V, vm: V) {
        let t = if d { 0x0040_0000 } else { 0 };
        self.word(0x1E20_2000 | t | (vm.0 as u32) << 16 | (vn.0 as u32) << 5);
    }

    /// `fcmp vn, #0.0`
    pub fn fcmp_zero(&mut self, d: bool, vn: V) {
        let t = if d { 0x0040_0000 } else { 0 };
        self.word(0x1E20_2008 | t | (vn.0 as u32) << 5);
    }

    /// `fmov vd, xn` (64 bits, `d`) or `fmov sd, wn` (32 bits).
    pub fn fmov_from_gp(&mut self, d: bool, vd: V, rn: Reg) {
        let base = if d { 0x9E67_0000 } else { 0x1E27_0000 };
        self.word(base | (rn.0 as u32) << 5 | vd.0 as u32);
    }

    /// `fmov xd, vn` / `fmov wd, sn`
    pub fn fmov_to_gp(&mut self, d: bool, rd: Reg, vn: V) {
        let base = if d { 0x9E66_0000 } else { 0x1E26_0000 };
        self.word(base | (vn.0 as u32) << 5 | rd.0 as u32);
    }

    /// `scvtf vd, xn`: a signed 64-bit integer to a double (`d`) or a float.
    pub fn scvtf(&mut self, d: bool, vd: V, rn: Reg) {
        let base = if d { 0x9E62_0000 } else { 0x9E22_0000 };
        self.word(base | (rn.0 as u32) << 5 | vd.0 as u32);
    }

    /// `fcvtzs xd, vn`: towards zero, saturating, NaN gives 0.
    pub fn fcvtzs(&mut self, d: bool, rd: Reg, vn: V) {
        let base = if d { 0x9E78_0000 } else { 0x9E38_0000 };
        self.word(base | (vn.0 as u32) << 5 | rd.0 as u32);
    }

    /// `fcvt`: a float to a double (`to_double`) or the reverse.
    pub fn fcvt(&mut self, to_double: bool, vd: V, vn: V) {
        let base = if to_double { 0x1E22_C000 } else { 0x1E62_4000 };
        self.word(base | (vn.0 as u32) << 5 | vd.0 as u32);
    }

    pub fn fsqrt(&mut self, d: bool, vd: V, vn: V) {
        self.fp2(0x1E21_C000, d, vd, vn);
    }

    // ---- Vectors (two 64-bit lanes) --------------------------------------------------------------

    fn vec3(&mut self, base: u32, vd: V, vn: V, vm: V) {
        self.word(base | (vm.0 as u32) << 16 | (vn.0 as u32) << 5 | vd.0 as u32);
    }

    /// `dup vd.2d, xn`
    pub fn vdup(&mut self, vd: V, rn: Reg) {
        self.word(0x4E08_0C00 | (rn.0 as u32) << 5 | vd.0 as u32);
    }

    pub fn vadd(&mut self, vd: V, vn: V, vm: V) {
        self.vec3(0x4EE0_8400, vd, vn, vm);
    }

    pub fn vsub(&mut self, vd: V, vn: V, vm: V) {
        self.vec3(0x6EE0_8400, vd, vn, vm);
    }

    pub fn vand(&mut self, vd: V, vn: V, vm: V) {
        self.vec3(0x4E20_1C00, vd, vn, vm);
    }

    pub fn vorr(&mut self, vd: V, vn: V, vm: V) {
        self.vec3(0x4EA0_1C00, vd, vn, vm);
    }

    pub fn veor(&mut self, vd: V, vn: V, vm: V) {
        self.vec3(0x6E20_1C00, vd, vn, vm);
    }

    /// `shl vd.2d, vn.2d, #k`
    pub fn vshl(&mut self, vd: V, vn: V, k: u32) {
        self.word(0x4F00_5400 | (64 + k) << 16 | (vn.0 as u32) << 5 | vd.0 as u32);
    }

    /// `ushr vd.2d, vn.2d, #k`
    pub fn vushr(&mut self, vd: V, vn: V, k: u32) {
        self.word(0x6F00_0400 | (128 - k) << 16 | (vn.0 as u32) << 5 | vd.0 as u32);
    }

    /// `addp dd, vn.2d`: the sum of the two lanes in the low lane of `vd`.
    pub fn vaddp(&mut self, vd: V, vn: V) {
        self.word(0x5EF1_B800 | (vn.0 as u32) << 5 | vd.0 as u32);
    }

    /// `mov vd.d[lane], xn`
    pub fn vins(&mut self, vd: V, lane: u32, rn: Reg) {
        self.word(0x4E08_1C00 | (lane << 4) << 16 | (rn.0 as u32) << 5 | vd.0 as u32);
    }

    /// `mov xd, vn.d[lane]`
    pub fn vmov_to_gp(&mut self, rd: Reg, vn: V, lane: u32) {
        self.word(0x4E08_3C00 | (lane << 4) << 16 | (vn.0 as u32) << 5 | rd.0 as u32);
    }

    /// `mov vd.16b, vn.16b`
    pub fn vmov(&mut self, vd: V, vn: V) {
        self.vec3(0x4EA0_1C00, vd, vn, vn);
    }

    // ---- Resolving ---------------------------------------------------------------------------------

    /// Resolves every reference. `text`, `rodata` and `data` are the virtual addresses the sections
    /// will have; the code itself starts at `text`.
    pub fn finish(&self, addrs: SectAddrs) -> Result<Vec<u8>, String> {
        let mut out = self.code.clone();
        for f in &self.fixups {
            let pc = addrs.text as i64 + f.at as i64;
            let target = match f.target {
                Target::Label(l) => {
                    let off = self.labels[l.0 as usize]
                        .ok_or_else(|| format!("label {} was never bound", l.0))?;
                    addrs.text as i64 + off as i64
                }
                Target::Sect(s, off) => {
                    let base = match s {
                        Sect::Text => addrs.text,
                        Sect::Rodata => addrs.rodata,
                        Sect::Data => addrs.data,
                        Sect::Idata => addrs.idata,
                    };
                    base as i64 + off as i64
                }
            };
            let mut word = u32::from_le_bytes(out[f.at..f.at + 4].try_into().unwrap());
            let too_far =
                || "a reference is too far away (a function bigger than 1 MiB?)".to_string();
            match f.kind {
                Kind::Branch26 => {
                    let rel = (target - pc) / 4;
                    if !(-(1 << 25)..1 << 25).contains(&rel) {
                        return Err(too_far());
                    }
                    word |= (rel as u32) & 0x03FF_FFFF;
                }
                Kind::Branch19 => {
                    let rel = (target - pc) / 4;
                    if !(-(1 << 18)..1 << 18).contains(&rel) {
                        return Err(too_far());
                    }
                    word |= ((rel as u32) & 0x7_FFFF) << 5;
                }
                Kind::Page => {
                    let rel = (target >> 12) - (pc >> 12);
                    if !(-(1 << 20)..1 << 20).contains(&rel) {
                        return Err(too_far());
                    }
                    let imm = rel as u32 & 0x1F_FFFF;
                    word |= (imm & 3) << 29 | (imm >> 2) << 5;
                }
                Kind::Lo12 => word |= ((target as u32) & 0xFFF) << 10,
            }
            out[f.at..f.at + 4].copy_from_slice(&word.to_le_bytes());
        }
        Ok(out)
    }
}
