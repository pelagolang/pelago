//! The x86-64 side of the machine IR (design in `docs/design/mir.md`): the opcodes, the register file
//! the shared allocator sees, and the spill moves. [`super::x86_select`] makes MIR from the IR and
//! [`super::x86_emit`] turns allocated MIR into bytes.
//!
//! Flags never live from one MIR instruction to the next. Every instruction that reads them is a macro
//! that sets them itself (`CMPJCC` is a `cmp` and a `jcc`, `CHK` is an operation and a `jo`), so the
//! allocator may put a move or a reload anywhere.
//!
//! Registers: the sixteen general registers are `PReg(0..16)` in hardware order and `xmm0` to `xmm15`
//! are `PReg(16..32)`. `rsp` and `rbp` are never named. `rax`, `r11`, `rcx` and `rdx` belong to the
//! allocator and to the emitter's macros (reloads, breaking cycles of copies, the transfer register of
//! a small `memcopy`); selected code names `rax`, `rcx` and `rdx` only where an instruction needs them
//! (`idiv`, a shift by `cl`, the return value, the argument registers). `r11` and `rax` are never an
//! operand of a parallel copy.

use super::mir::*;

pub const RAX: PReg = PReg(0);
pub const RCX: PReg = PReg(1);
pub const RDX: PReg = PReg(2);
pub const RBX: PReg = PReg(3);
pub const RSI: PReg = PReg(6);
pub const RDI: PReg = PReg(7);
pub const R8: PReg = PReg(8);
pub const R9: PReg = PReg(9);
pub const R10: PReg = PReg(10);
pub const R11: PReg = PReg(11);
pub const R12: PReg = PReg(12);
pub const R13: PReg = PReg(13);
pub const R14: PReg = PReg(14);
pub const R15: PReg = PReg(15);

/// `xmm<n>`.
pub const fn xmm(n: u8) -> PReg {
    PReg(16 + n)
}

/// The registers that carry integer arguments, in order.
pub const INT_ARGS: [PReg; 6] = [RDI, RSI, RDX, RCX, R8, R9];

/// Opcodes.
pub mod op {
    use super::super::mir::FIRST_TARGET_OP as B;

    // Two-address integer operations `d = d op s`; `s` is a register, an immediate or a memory operand.
    pub const ADD: u16 = B;
    pub const SUB: u16 = B + 1;
    pub const AND: u16 = B + 2;
    pub const OR: u16 = B + 3;
    pub const XOR: u16 = B + 4;
    /// `d = d * s` (register), 64 bits.
    pub const IMUL: u16 = B + 5;
    /// `d = s * imm`.
    pub const IMULI: u16 = B + 6;
    /// `d = address` (a memory operand).
    pub const LEA: u16 = B + 7;
    pub const NEG: u16 = B + 8;
    pub const NOT: u16 = B + 9;
    /// `d = d << n`, `>> n` (logical), `>> n` (arithmetic): `[d UseDef, Imm n]`.
    pub const SHL_I: u16 = B + 10;
    pub const SHR_I: u16 = B + 11;
    pub const SAR_I: u16 = B + 12;
    /// Cut `d` to `Imm bits` (8, 16 or 32) and zero-extend: `[d UseDef, Imm bits]`.
    pub const MASK: u16 = B + 13;
    /// `d = extend(s)` from `Imm size` bytes to 64 bits, signed when `Imm` 1: `[d Def, s Use, Imm size, Imm signed]`.
    pub const EXT: u16 = B + 14;
    /// `d = [mem]` of `Imm size` bytes, zero-extended: `[d Def, Mem, Imm size]`.
    pub const LOAD: u16 = B + 15;
    /// `[mem] = s` of `Imm size` bytes: `[Mem, s Use|Imm, Imm size]`.
    pub const STORE: u16 = B + 16;
    /// `cmp a, b; jcc cc, target`: `[Imm cc, a Use, b Use|Imm, Block]`.
    pub const CMPJCC: u16 = B + 17;
    /// `test a, a; jcc cc, target`: `[Imm cc, a Use, Block]`.
    pub const TESTJCC: u16 = B + 18;
    /// `jmp target`: `[Block]`.
    pub const JMP: u16 = B + 19;
    /// `cmp a, b; setcc d; movzx d, d`: `[d Def, Imm cc, a Use, b Use|Imm]`.
    pub const CMPSET: u16 = B + 20;
    /// `cmp a, b; cmovcc d, s`: `[d UseDef, s Use, Imm cc, a Use, b Use|Imm]`.
    pub const CMPCMOV: u16 = B + 21;
    /// `test c, c; cmovcc d, s`: `[d UseDef, s Use, Imm cc, c Use]`.
    pub const TESTCMOV: u16 = B + 22;
    /// A 64-bit operation that stops the program on overflow: `[d UseDef, s Use|Imm, Imm kind]` with
    /// the kinds below, then `jcc` to the overflow stop.
    pub const CHK: u16 = B + 23;
    pub const CHK_ADD_S: i64 = 0;
    pub const CHK_SUB_S: i64 = 1;
    pub const CHK_MUL_S: i64 = 2;
    pub const CHK_ADD_U: i64 = 3;
    pub const CHK_SUB_U: i64 = 4;
    /// Unsigned multiply, `rdx:rax = rax * s`, stopping on carry: `[rax UseDef, rdx Def, s Use]`.
    pub const MULU_CHK: u16 = B + 24;
    /// Division with the checks of the IR: `[rax UseDef, rdx Def, divisor Use, Imm kind]` with the
    /// kind bits below; the quotient is in `rax`, the remainder in `rdx`.
    pub const DIV: u16 = B + 25;
    pub const DIV_SIGNED: i64 = 1;
    pub const DIV_REM: i64 = 2;
    /// A 64-bit operand (`MIN / -1` wraps instead of trapping).
    pub const DIV_WIDE: i64 = 4;
    /// High half of a product: `[rax UseDef, rdx Def, s Use, Imm signed]` (`mul` or `imul`, one operand).
    pub const MULHI: u16 = B + 26;
    /// A shift by `cl` that stops when the count is the width or more:
    /// `[d UseDef, rcx Use, Imm shift, Imm width]` (shift: 0 `shl`, 1 `shr`, 2 `sar`).
    pub const SHIFT_CL: u16 = B + 27;
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
    /// Epilogue and `ret`; `[rax Use]` when a value is returned.
    pub const RET: u16 = B + 31;
    /// Stop the program with the message number `Imm`: `[Imm n]`.
    pub const STOP: u16 = B + 32;
    /// Jump to a shared stop routine: `[Sym]`.
    pub const JMPSYM: u16 = B + 33;
    /// Zero `Imm` bytes of the frame object (`[Frame, Imm size]`).
    pub const ZERO: u16 = B + 34;
    /// Copy `Imm` bytes (at most 64) between two memory operands through `rax`: `[Mem to, Mem from, Imm]`.
    pub const MEMCOPY: u16 = B + 35;
    /// `rep movsb`: `[rdi UseDef, rsi UseDef, rcx UseDef]`.
    pub const REP_MOVSB: u16 = B + 36;
    /// `d = address of static data number Imm`.
    pub const DATAADDR: u16 = B + 37;
    /// `d = address of the function Sym`.
    pub const FUNCADDR: u16 = B + 38;
    /// `cmp a, b; jcc cc, stop`: `[Imm cc, a Use, b Use|Imm, Sym]`.
    pub const CMPSTOP: u16 = B + 39;
    /// Spill moves of floats and vectors: `[d Def, Mem]` and `[Mem, s Use]`.
    pub const FLOAD: u16 = B + 40;
    pub const FSTORE: u16 = B + 41;
    pub const VLOAD: u16 = B + 42;
    pub const VSTORE: u16 = B + 43;
    pub const COUNT: u16 = B + 44;
}

/// Symbols: the shared stop routines, then the functions of the module.
pub const SYM_DIV0: u32 = 0;
pub const SYM_SHIFT: u32 = 1;
pub const SYM_OVERFLOW: u32 = 2;
pub const SYM_UNREACHABLE: u32 = 3;
pub const SYM_FUNC: u32 = 16;

/// Condition codes as numbers in an `Imm` operand: the hardware codes of [`super::x86::Cc`].
pub use super::x86::Cc;

pub fn cc_num(cc: Cc) -> i64 {
    cc as i64
}

pub fn cc_of(n: i64) -> Cc {
    [
        Cc::O,
        Cc::No,
        Cc::B,
        Cc::Ae,
        Cc::E,
        Cc::Ne,
        Cc::Be,
        Cc::A,
        Cc::S,
        Cc::Ns,
        Cc::P,
        Cc::Np,
        Cc::L,
        Cc::Ge,
        Cc::Le,
        Cc::G,
    ][n as usize]
}

/// Everything a call destroys: the caller-saved general registers and every SSE register.
pub fn call_clobbers() -> RegSet {
    let mut s = RegSet::default();
    for r in [RAX, RCX, RDX, RSI, RDI, R8, R9, R10, R11] {
        s.insert(r);
    }
    for n in 0..16 {
        s.insert(xmm(n));
    }
    s
}

pub struct X86 {
    regs: Vec<RegInfo>,
    int: Vec<PReg>,
    float: Vec<PReg>,
    int_scratch: Vec<PReg>,
    float_scratch: Vec<PReg>,
}

const GPR_NAMES: [&str; 16] = [
    "rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi", "r8", "r9", "r10", "r11", "r12", "r13",
    "r14", "r15",
];
const XMM_NAMES: [&str; 16] = [
    "xmm0", "xmm1", "xmm2", "xmm3", "xmm4", "xmm5", "xmm6", "xmm7", "xmm8", "xmm9", "xmm10",
    "xmm11", "xmm12", "xmm13", "xmm14", "xmm15",
];

impl X86 {
    pub fn new() -> X86 {
        let mut regs = Vec::new();
        for (i, name) in GPR_NAMES.iter().enumerate() {
            regs.push(RegInfo {
                name,
                class: Class::Int,
                callee_saved: matches!(i, 3 | 5 | 12..=15),
            });
        }
        for name in XMM_NAMES {
            regs.push(RegInfo {
                name,
                class: Class::Float,
                callee_saved: false,
            });
        }
        X86 {
            regs,
            // Caller-saved first (free when nothing is called while the value lives), then the
            // callee-saved ones.
            int: vec![R10, R8, R9, RSI, RDI, RBX, R12, R13, R14, R15],
            float: (0..13).map(xmm).collect(),
            int_scratch: vec![RAX, R11, RCX, RDX],
            float_scratch: vec![xmm(15), xmm(14), xmm(13)],
        }
    }
}

impl Default for X86 {
    fn default() -> X86 {
        X86::new()
    }
}

fn mem_frame(f: FrameId) -> Operand {
    Operand::Mem {
        base: MemBase::Frame(f),
        index: None,
        disp: 0,
    }
}

impl Target for X86 {
    fn regs(&self) -> &[RegInfo] {
        &self.regs
    }

    fn allocatable(&self, c: Class) -> &[PReg] {
        match c {
            Class::Int => &self.int,
            Class::Float | Class::Vec => &self.float,
        }
    }

    fn scratch(&self, c: Class) -> &[PReg] {
        match c {
            Class::Int => &self.int_scratch,
            Class::Float | Class::Vec => &self.float_scratch,
        }
    }

    fn op_name(&self, o: u16) -> &'static str {
        const NAMES: [&str; 44] = [
            "add",
            "sub",
            "and",
            "or",
            "xor",
            "imul",
            "imuli",
            "lea",
            "neg",
            "not",
            "shl",
            "shr",
            "sar",
            "mask",
            "ext",
            "load",
            "store",
            "cmpjcc",
            "testjcc",
            "jmp",
            "cmpset",
            "cmpcmov",
            "testcmov",
            "chk",
            "mulu_chk",
            "div",
            "mulhi",
            "shift_cl",
            "call",
            "callr",
            "callrt",
            "ret",
            "stop",
            "jmpsym",
            "zero",
            "memcopy",
            "rep_movsb",
            "dataaddr",
            "funcaddr",
            "cmpstop",
            "fload",
            "fstore",
            "vload",
            "vstore",
        ];
        NAMES
            .get(o.wrapping_sub(op::ADD) as usize)
            .copied()
            .unwrap_or("?")
    }

    fn mov(&self, _c: Class, dst: PReg, src: PReg) -> MInst {
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
            Class::Float => MInst::new(op::FLOAD, vec![d, mem_frame(f)]),
            Class::Vec => MInst::new(op::VLOAD, vec![d, mem_frame(f)]),
        }
    }

    fn store(&self, c: Class, f: FrameId, src: PReg) -> MInst {
        let s = Operand::use_(Reg::Phys(src));
        match c {
            Class::Int => MInst::new(op::STORE, vec![mem_frame(f), s, Operand::Imm(8)]),
            Class::Float => MInst::new(op::FSTORE, vec![mem_frame(f), s]),
            Class::Vec => MInst::new(op::VSTORE, vec![mem_frame(f), s]),
        }
    }
}
