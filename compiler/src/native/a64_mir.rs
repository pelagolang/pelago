//! The AArch64 side of the machine IR (design in `docs/design/mir.md`): the opcodes, the register file
//! the shared allocator sees, and the spill moves. [`super::a64_select`] makes MIR from the IR and
//! [`super::a64_emit`] turns allocated MIR into bytes.
//!
//! Integer operations are three-address (`d = a op b`): the allocator needs no copy to keep `a`. As on
//! x86-64, flags never live from one MIR instruction to the next: every instruction that reads them is
//! a macro that sets them itself (`CMPJCC` is a `cmp` and a `b.cond`, `CHK` is an `adds` and a `b.vs`).
//!
//! Registers: `x0` to `x28` are `PReg(0..29)` in hardware order and `v0` to `v31` are `PReg(32..64)`.
//! `x16` and `x17` belong to the assembler (`a64.rs` builds addresses and constants in them), `x18` is
//! the platform register on Windows and macOS, `x29` / `x30` / `sp` are never named. `x9` to `x12`
//! belong to the allocator and to the emitter's macros (reloads, breaking cycles of copies, the quotient
//! of a remainder); selected code names them only through `clobbers`.

use super::mir::*;

/// `x<n>`.
pub const fn x(n: u8) -> PReg {
    PReg(n)
}

/// `v<n>`.
pub const fn v(n: u8) -> PReg {
    PReg(32 + n)
}

/// The registers that carry integer arguments, in order.
pub const INT_ARGS: [PReg; 8] = [x(0), x(1), x(2), x(3), x(4), x(5), x(6), x(7)];
/// The register a call through a pointer uses.
pub const CALL_TARGET: PReg = x(15);

/// Opcodes.
pub mod op {
    use super::super::mir::FIRST_TARGET_OP as B;

    // Three-address integer operations `d = a op b`; `b` is a register or an immediate (one that the
    // instruction can encode: the selector checks).
    pub const ADD: u16 = B;
    pub const SUB: u16 = B + 1;
    pub const AND: u16 = B + 2;
    pub const OR: u16 = B + 3;
    pub const XOR: u16 = B + 4;
    /// `d = a * b` (registers), 64 bits.
    pub const MUL: u16 = B + 5;
    /// `d = a * b + c` and `d = c - a * b`: `[d Def, a Use, b Use, c Use]`.
    pub const MADD: u16 = B + 6;
    pub const MSUB: u16 = B + 7;
    /// `d = address` (a memory operand).
    pub const LEA: u16 = B + 8;
    pub const NEG: u16 = B + 9;
    pub const NOT: u16 = B + 10;
    /// `d = s << n`, `>> n` (logical), `>> n` (arithmetic): `[d Def, s Use, Imm n]`.
    pub const SHL_I: u16 = B + 11;
    pub const SHR_I: u16 = B + 12;
    pub const SAR_I: u16 = B + 13;
    /// `d = s` cut to `Imm bits` (8, 16 or 32) and zero-extended: `[d Def, s Use, Imm bits]`.
    pub const MASK: u16 = B + 14;
    /// `d = extend(s)` from `Imm size` bytes to 64 bits, signed when `Imm` 1: `[d Def, s Use, Imm size, Imm signed]`.
    pub const EXT: u16 = B + 15;
    /// `d = [mem]` of `Imm size` bytes, zero-extended: `[d Def, Mem, Imm size]`.
    pub const LOAD: u16 = B + 16;
    /// `[mem] = s` of `Imm size` bytes: `[Mem, s Use|Imm 0, Imm size]`.
    pub const STORE: u16 = B + 17;
    /// `cmp a, b; b.cc target`: `[Imm cc, a Use, b Use|Imm, Block]`.
    pub const CMPJCC: u16 = B + 18;
    /// `cbz` / `cbnz`: `[Imm cc (Eq or Ne), a Use, Block]`.
    pub const TESTJCC: u16 = B + 19;
    /// `b target`: `[Block]`.
    pub const JMP: u16 = B + 20;
    /// `cmp a, b; cset d, cc`: `[d Def, Imm cc, a Use, b Use|Imm]`.
    pub const CMPSET: u16 = B + 21;
    /// `cmp a, b; csel d, t, f, cc`: `[d Def, t Use, f Use, Imm cc, a Use, b Use|Imm]`.
    pub const CMPSEL: u16 = B + 22;
    /// `cmp c, 0; csel d, t, f, ne`: `[d Def, t Use, f Use, c Use]`.
    pub const TESTSEL: u16 = B + 23;
    /// A 64-bit operation that stops the program on overflow: `[d Def, a Use, b Use|Imm, Imm kind]`
    /// with the kinds below, then the jump to the overflow stop (through a cold stub when a test
    /// follows).
    pub const CHK: u16 = B + 24;
    pub const CHK_ADD_S: i64 = 0;
    pub const CHK_SUB_S: i64 = 1;
    pub const CHK_MUL_S: i64 = 2;
    pub const CHK_ADD_U: i64 = 3;
    pub const CHK_SUB_U: i64 = 4;
    pub const CHK_MUL_U: i64 = 5;
    /// A signed product by 3 (`x + 2x`, checked by the adder): `[d Def, a Use, Imm 3 kind...]` is a
    /// `CHK` with this kind and `b` = `Imm 3`.
    pub const CHK_MUL3_S: i64 = 6;
    /// Division with the checks of the IR: `[d Def, a Use, b Use, Imm kind]` with the kind bits below.
    pub const DIV: u16 = B + 25;
    pub const DIV_SIGNED: i64 = 1;
    pub const DIV_REM: i64 = 2;
    /// High half of a product: `[d Def, a Use, b Use, Imm signed]`.
    pub const MULHI: u16 = B + 26;
    /// A shift by a register that stops when the count is the width or more:
    /// `[d Def, a Use, b Use, Imm shift, Imm width]` (shift: 0 `lsl`, 1 `lsr`, 2 `asr`).
    pub const SHIFT_CHK: u16 = B + 27;
    /// A call of the function `Sym`; the argument registers are listed as uses.
    pub const CALL: u16 = B + 28;
    /// A call through a register: `[r Use, ...]`.
    pub const CALLR: u16 = B + 29;
    /// A call of a runtime routine: `[Imm which, ...]` with the numbers below.
    pub const CALLRT: u16 = B + 30;
    pub const RT_ALLOC: i64 = 0;
    pub const RT_FREE: i64 = 1;
    pub const RT_REALLOC: i64 = 2;
    pub const RT_PRINT: i64 = 3;
    pub const RT_CTX_INIT: i64 = 4;
    pub const RT_CTX_SWITCH: i64 = 5;
    pub const RT_MEMCPY: i64 = 6;
    pub const RT_FMOD: i64 = 7;
    /// Epilogue and `ret`; `[x0 Use]` or `[d0 Use]` when a value is returned.
    pub const RET: u16 = B + 31;
    /// Stop the program with the message number `Imm`: `[Imm n]`.
    pub const STOP: u16 = B + 32;
    /// Jump to a shared stop routine: `[Sym]`.
    pub const JMPSYM: u16 = B + 33;
    /// Zero `Imm` bytes of the frame object (`[Frame, Imm size]`).
    pub const ZERO: u16 = B + 34;
    /// Copy `Imm` bytes (at most 64) between two memory operands: `[Mem to, Mem from, Imm]`.
    pub const MEMCOPY: u16 = B + 35;
    /// `d = address of static data number Imm`.
    pub const DATAADDR: u16 = B + 36;
    /// `d = address of the function Sym`.
    pub const FUNCADDR: u16 = B + 37;
    /// `cmp a, b; b.cc stop`: `[Imm cc, a Use, b Use|Imm, Sym]`.
    pub const CMPSTOP: u16 = B + 38;
    pub const COUNT: u16 = B + 39;
}

/// Symbols: the shared stop routines, then the functions of the module.
pub const SYM_DIV0: u32 = 0;
pub const SYM_SHIFT: u32 = 1;
pub const SYM_OVERFLOW: u32 = 2;
pub const SYM_UNREACHABLE: u32 = 3;
pub const SYM_FUNC: u32 = 16;

pub use super::a64::Cond;

pub fn cc_num(cc: Cond) -> i64 {
    cc as i64
}

pub fn cc_of(n: i64) -> Cond {
    use Cond::*;
    [Eq, Ne, Hs, Lo, Mi, Pl, Vs, Vc, Hi, Ls, Ge, Lt, Gt, Le][n as usize]
}

/// Everything a call destroys: the caller-saved general registers and the caller-saved vector ones.
pub fn call_clobbers() -> RegSet {
    let mut s = RegSet::default();
    for n in 0..16 {
        s.insert(x(n));
    }
    for n in (0..8).chain(16..32) {
        s.insert(v(n));
    }
    s
}

pub struct A64 {
    regs: Vec<RegInfo>,
    int: Vec<PReg>,
    float: Vec<PReg>,
    vec: Vec<PReg>,
    int_scratch: Vec<PReg>,
    float_scratch: Vec<PReg>,
}

impl A64 {
    pub fn new() -> A64 {
        let mut regs = Vec::new();
        for i in 0..32u8 {
            regs.push(RegInfo {
                name: X_NAMES[i as usize],
                class: Class::Int,
                callee_saved: (19..=28).contains(&i),
            });
        }
        for i in 0..32u8 {
            regs.push(RegInfo {
                name: V_NAMES[i as usize],
                class: Class::Float,
                // Only the low 64 bits survive a call: right for floats, not for vectors.
                callee_saved: (8..=15).contains(&i),
            });
        }
        let mut int: Vec<PReg> = vec![x(13), x(14), x(8)];
        int.extend((0..8).rev().map(x));
        int.extend((19..=28).map(x));
        // The caller-saved ones first (free when nothing is called while the value lives), then the
        // callee-saved ones.
        let mut float: Vec<PReg> = (16..29).map(v).collect();
        float.extend((0..8).rev().map(v));
        let vec = float.clone();
        float.extend((8..16).map(v));
        A64 {
            regs,
            int,
            float,
            vec,
            int_scratch: vec![x(9), x(10), x(11), x(12)],
            float_scratch: vec![v(29), v(30), v(31)],
        }
    }
}

impl Default for A64 {
    fn default() -> A64 {
        A64::new()
    }
}

const X_NAMES: [&str; 32] = [
    "x0", "x1", "x2", "x3", "x4", "x5", "x6", "x7", "x8", "x9", "x10", "x11", "x12", "x13", "x14",
    "x15", "x16", "x17", "x18", "x19", "x20", "x21", "x22", "x23", "x24", "x25", "x26", "x27",
    "x28", "x29", "x30", "sp",
];
const V_NAMES: [&str; 32] = [
    "v0", "v1", "v2", "v3", "v4", "v5", "v6", "v7", "v8", "v9", "v10", "v11", "v12", "v13", "v14",
    "v15", "v16", "v17", "v18", "v19", "v20", "v21", "v22", "v23", "v24", "v25", "v26", "v27",
    "v28", "v29", "v30", "v31",
];

fn mem_frame(f: FrameId) -> Operand {
    Operand::Mem {
        base: MemBase::Frame(f),
        index: None,
        disp: 0,
    }
}

impl Target for A64 {
    fn regs(&self) -> &[RegInfo] {
        &self.regs
    }

    fn allocatable(&self, c: Class) -> &[PReg] {
        match c {
            Class::Int => &self.int,
            Class::Float => &self.float,
            Class::Vec => &self.vec,
        }
    }

    fn scratch(&self, c: Class) -> &[PReg] {
        match c {
            Class::Int => &self.int_scratch,
            Class::Float | Class::Vec => &self.float_scratch,
        }
    }

    fn op_name(&self, o: u16) -> &'static str {
        const NAMES: [&str; 39] = [
            "add",
            "sub",
            "and",
            "orr",
            "eor",
            "mul",
            "madd",
            "msub",
            "lea",
            "neg",
            "mvn",
            "lsl",
            "lsr",
            "asr",
            "mask",
            "ext",
            "load",
            "store",
            "cmpjcc",
            "testjcc",
            "jmp",
            "cmpset",
            "cmpsel",
            "testsel",
            "chk",
            "div",
            "mulhi",
            "shift_chk",
            "call",
            "callr",
            "callrt",
            "ret",
            "stop",
            "jmpsym",
            "zero",
            "memcopy",
            "dataaddr",
            "funcaddr",
            "cmpstop",
        ];
        NAMES
            .get(o.wrapping_sub(op::ADD) as usize)
            .copied()
            .unwrap_or("?")
    }

    fn jump(&self, to: u32) -> MInst {
        MInst::new(op::JMP, vec![Operand::Block(to)])
            .branch()
            .nofall()
    }

    fn commutes(&self, _inst: &MInst) -> bool {
        false
    }

    fn mov(&self, c: Class, dst: PReg, src: PReg) -> MInst {
        let _ = c;
        MInst::copy(Reg::Phys(dst), Operand::use_(Reg::Phys(src)))
    }

    fn mov_imm(&self, dst: PReg, imm: i64) -> MInst {
        MInst::copy(Reg::Phys(dst), Operand::Imm(imm))
    }

    fn lea(&self, dst: PReg, f: FrameId, disp: i32) -> MInst {
        MInst::copy(Reg::Phys(dst), Operand::Frame(f, disp))
    }

    fn load(&self, c: Class, dst: PReg, f: FrameId) -> MInst {
        let d = Operand::def(Reg::Phys(dst));
        match c {
            Class::Int => MInst::new(op::LOAD, vec![d, mem_frame(f), Operand::Imm(8)]),
            Class::Float | Class::Vec => {
                unreachable!("float spills arrive with the float selector")
            }
        }
    }

    fn store(&self, c: Class, f: FrameId, src: PReg) -> MInst {
        let s = Operand::use_(Reg::Phys(src));
        match c {
            Class::Int => MInst::new(op::STORE, vec![mem_frame(f), s, Operand::Imm(8)]),
            Class::Float | Class::Vec => {
                unreachable!("float spills arrive with the float selector")
            }
        }
    }
}
