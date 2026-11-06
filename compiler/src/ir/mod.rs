//! The intermediate representation (`docs/design/ir.md`): SSA in basic blocks.
//!
//! A [`Func`] is a list of blocks. A block has parameters (instead of phi nodes), a list of
//! instructions and one terminator. Every value is assigned once. The entry block's parameters are
//! the function's parameters. Everything is stored in flat vectors and referred to by number.
//!
//! This first version has scalar types and pointers; aggregates arrive with the lowering of
//! structs and enums.

mod interp;
pub mod opt;
mod print;
#[cfg(test)]
mod tests;
mod verify;

pub(crate) use interp::fold;
pub use interp::{run, run_capture, run_full, Capture, Outcome, Val};
pub use print::{print_func, print_module};
pub use verify::{verify_func, verify_module};

use crate::span::Span;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct Value(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct BlockId(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct FuncId(pub u32);

/// The type of a value in the IR.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum IrTy {
    /// An integer of 8, 16, 32 or 64 bits. Signedness lives in the operations, not the type.
    Int(u8),
    F32,
    F64,
    Bool,
    Ptr,
    Void,
    /// Two 64-bit integer lanes (an SSE2 register). Made by [`Op::VUn`] and [`Op::VBin`], read by a store or
    /// a reduction; never in memory layouts of the language.
    V2I64,
}

impl IrTy {
    pub const I64: IrTy = IrTy::Int(64);
    pub const I32: IrTy = IrTy::Int(32);
    pub const I8: IrTy = IrTy::Int(8);

    pub fn is_int(self) -> bool {
        matches!(self, IrTy::Int(_))
    }

    /// Size in bytes in memory.
    pub fn size(self) -> u32 {
        match self {
            IrTy::Int(b) => b as u32 / 8,
            IrTy::F32 => 4,
            IrTy::F64 | IrTy::Ptr => 8,
            IrTy::V2I64 => 16,
            IrTy::Bool => 1,
            IrTy::Void => 0,
        }
    }

    pub fn is_float(self) -> bool {
        matches!(self, IrTy::F32 | IrTy::F64)
    }

    pub fn name(self) -> String {
        match self {
            IrTy::Int(b) => format!("i{b}"),
            IrTy::F32 => "f32".into(),
            IrTy::F64 => "f64".into(),
            IrTy::Bool => "bool".into(),
            IrTy::Ptr => "ptr".into(),
            IrTy::Void => "void".into(),
            IrTy::V2I64 => "v2i64".into(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Const {
    Int(i128),
    Float(f64),
    Bool(bool),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    /// Signed division and remainder; unsigned forms are separate.
    Div,
    Rem,
    UDiv,
    URem,
    /// The high half of the full product: `(a * b) >> bits`, signed and unsigned. Integers up to 64 bits.
    MulHi,
    UMulHi,
    And,
    Or,
    Xor,
    Shl,
    /// Arithmetic (signed) and logical (unsigned) right shifts.
    Shr,
    UShr,
}

impl BinOp {
    pub fn name(self) -> &'static str {
        match self {
            BinOp::Add => "add",
            BinOp::Sub => "sub",
            BinOp::Mul => "mul",
            BinOp::Div => "div",
            BinOp::Rem => "rem",
            BinOp::UDiv => "udiv",
            BinOp::URem => "urem",
            BinOp::MulHi => "mulhi",
            BinOp::UMulHi => "umulhi",
            BinOp::And => "and",
            BinOp::Or => "or",
            BinOp::Xor => "xor",
            BinOp::Shl => "shl",
            BinOp::Shr => "shr",
            BinOp::UShr => "ushr",
        }
    }

    pub fn is_bitwise(self) -> bool {
        matches!(
            self,
            BinOp::And | BinOp::Or | BinOp::Xor | BinOp::Shl | BinOp::Shr | BinOp::UShr
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    /// Unsigned orderings.
    ULt,
    ULe,
    UGt,
    UGe,
}

impl CmpOp {
    /// The comparison that is true exactly when this one is false (for integers).
    pub fn inverse(self) -> CmpOp {
        match self {
            CmpOp::Eq => CmpOp::Ne,
            CmpOp::Ne => CmpOp::Eq,
            CmpOp::Lt => CmpOp::Ge,
            CmpOp::Ge => CmpOp::Lt,
            CmpOp::Le => CmpOp::Gt,
            CmpOp::Gt => CmpOp::Le,
            CmpOp::ULt => CmpOp::UGe,
            CmpOp::UGe => CmpOp::ULt,
            CmpOp::ULe => CmpOp::UGt,
            CmpOp::UGt => CmpOp::ULe,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            CmpOp::Eq => "eq",
            CmpOp::Ne => "ne",
            CmpOp::Lt => "lt",
            CmpOp::Le => "le",
            CmpOp::Gt => "gt",
            CmpOp::Ge => "ge",
            CmpOp::ULt => "ult",
            CmpOp::ULe => "ule",
            CmpOp::UGt => "ugt",
            CmpOp::UGe => "uge",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CastKind {
    /// Widen an integer, filling with zeros or with the sign.
    ZExt,
    SExt,
    Trunc,
    IntToFloat,
    FloatToInt,
    FloatResize,
    /// Reinterpret between a pointer and a 64-bit integer.
    PtrToInt,
    IntToPtr,
}

impl CastKind {
    pub fn name(self) -> &'static str {
        match self {
            CastKind::ZExt => "zext",
            CastKind::SExt => "sext",
            CastKind::Trunc => "trunc",
            CastKind::IntToFloat => "itof",
            CastKind::FloatToInt => "ftoi",
            CastKind::FloatResize => "fresize",
            CastKind::PtrToInt => "ptrtoint",
            CastKind::IntToPtr => "inttoptr",
        }
    }
}

/// When a `CheckedIf` may stop.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Test {
    /// A bool and the value it must have.
    Bool(Value, bool),
    /// A comparison that holds; made again where it is needed, so no bool is kept.
    Cmp(CmpOp, Value, Value),
}

impl Test {
    pub fn for_each_value(&self, mut f: impl FnMut(Value)) {
        match self {
            Test::Bool(c, _) => f(*c),
            Test::Cmp(_, a, b) => {
                f(*a);
                f(*b);
            }
        }
    }

    pub fn map_values(&mut self, mut f: impl FnMut(Value) -> Value) {
        match self {
            Test::Bool(c, _) => *c = f(*c),
            Test::Cmp(_, a, b) => {
                *a = f(*a);
                *b = f(*b);
            }
        }
    }
}

/// One-operand vector operations. Shift counts are below 64.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VUn {
    /// An `i64` copied to both lanes.
    Splat,
    /// Each lane shifted left or logically right by a constant.
    Shl(u8),
    UShr(u8),
    /// The wrapping sum of the lanes, an `i64`.
    ReduceAdd,
}

/// Two-operand vector operations, all lane by lane and wrapping.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VBin {
    /// Two `i64`s, the first in lane 0 and the second in lane 1.
    Build,
    Add,
    Sub,
    And,
    Or,
    Xor,
}

impl VBin {
    pub fn name(self) -> &'static str {
        match self {
            VBin::Build => "vbuild",
            VBin::Add => "vadd",
            VBin::Sub => "vsub",
            VBin::And => "vand",
            VBin::Or => "vor",
            VBin::Xor => "vxor",
        }
    }
}

/// One-operand float operations that the hardware does exactly: the result is the correctly rounded
/// (square root) or exact (rounding to a whole number) value, the same on every target.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FUn {
    /// Square root; a negative number gives NaN.
    Sqrt,
    /// The largest whole number not above the operand.
    Floor,
    /// The smallest whole number not below the operand.
    Ceil,
}

impl FUn {
    pub fn name(self) -> &'static str {
        match self {
            FUn::Sqrt => "fsqrt",
            FUn::Floor => "ffloor",
            FUn::Ceil => "fceil",
        }
    }

    /// The value for an `f64` operand (an `f32` is widened first and narrowed after: both steps are exact
    /// for these operations).
    pub fn apply(self, x: f64) -> f64 {
        match self {
            FUn::Sqrt => x.sqrt(),
            FUn::Floor => x.floor(),
            FUn::Ceil => x.ceil(),
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum Op {
    Const(IrTy, Const),
    Bin(BinOp, Value, Value),
    /// Like `Bin` for `Add`, `Sub`, `Mul`, but stops the program on overflow (signed: `true`).
    Checked(BinOp, bool, Value, Value),
    /// `Checked` that only stops when the test holds: the other way it is the wrapping operation. Made when
    /// a branch's side runs ahead of the branch.
    CheckedIf(BinOp, bool, Test, Value, Value),
    Cmp(CmpOp, Value, Value),
    /// The second value when the bool is true, else the third (both are already computed).
    Select(Value, Value, Value),
    /// `a * b + c` with a single rounding (floats only): the same result on every target.
    Fma(Value, Value, Value),
    Neg(Value),
    /// A float square root or rounding to a whole number (see `FUn`).
    FUn(FUn, Value),
    Not(Value),
    VUn(VUn, Value),
    VBin(VBin, Value, Value),
    Cast(CastKind, Value, IrTy),
    Call(FuncId, Vec<Value>),
    /// Reserve stack space of this size and alignment (in bytes); gives its address.
    Slot(u32, u32),
    /// An address moved by a number of bytes.
    PtrAdd(Value, Value),
    /// Copy this many bytes from the second address to the first (the areas do not overlap).
    MemCopy(Value, Value, Value),
    /// The address of the n-th piece of static data (a string literal, a table).
    DataAddr(u32),
    /// The address of a function, for a call through a pointer.
    FuncAddr(FuncId),
    /// Call through a function address, with the signature the callee must have.
    CallPtr(Vec<IrTy>, IrTy, Value, Vec<Value>),
    /// Write bytes (address, length) to the program's output.
    Print(Value, Value),
    Load(IrTy, Value),
    Store(Value, Value),
    /// Heap memory of the given size in bytes; gives its address.
    Alloc(Value),
    /// Change the size of a heap block (a null address allocates): gives the block's address, which may
    /// be a new one. The first bytes up to the smaller size are kept, the rest is zero; the old
    /// address is dead. A large block can move without copying.
    Realloc(Value, Value),
    Free(Value),
    /// Prepares a context for `CtxSwitch`: a stack of `size` bytes at `base` (addresses grow down from the end
    /// in compiled code) and the function it starts in, `code(env)`, which never returns. Gives the
    /// context's handle. Nothing runs yet.
    CtxInit(Value, Value, Value, Value),
    /// Saves the running context's handle at the address in the first value and continues the context
    /// whose handle is the second. Comes back when something switches to the saved handle.
    CtxSwitch(Value, Value),
}

#[derive(Clone, PartialEq, Debug)]
pub struct Inst {
    /// The value this makes, when it makes one.
    pub result: Option<Value>,
    pub op: Op,
    pub span: Span,
}

/// Where a jump goes and the values it hands to the block's parameters.
#[derive(Clone, PartialEq, Debug)]
pub struct Target {
    pub block: BlockId,
    pub args: Vec<Value>,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Term {
    Jump(Target),
    Branch(Value, Target, Target),
    Return(Option<Value>),
    /// Stop the program with a message (a bug was found: overflow, bounds, `panic`).
    Stop(String),
    Unreachable,
}

#[derive(Clone, Default, Debug)]
pub struct Block {
    pub params: Vec<Value>,
    pub insts: Vec<usize>,
    pub term: Option<Term>,
    /// The header of a loop that finishes what a vectorised loop left over (or is the vector loop):
    /// the unroller and the vectoriser leave it alone.
    pub tail: bool,
    /// The checked copy of a loop that `opt::bce` made a check-free copy of: it runs only when the
    /// guard in front of the loops fails, so `@checkfree` does not count what it still tests.
    pub slow: bool,
}

#[derive(Clone, Debug)]
pub struct Func {
    pub name: String,
    pub params: Vec<IrTy>,
    pub ret: IrTy,
    /// The type of each value.
    pub values: Vec<IrTy>,
    pub blocks: Vec<Block>,
    pub insts: Vec<Inst>,
    /// Set for a function written `@checkfree`: the module and the declaration it came from. The
    /// optimiser keeps it a function of its own, and the build fails when an index test is left on
    /// its fast path (`opt::kept_checks`).
    pub checkfree: Option<(u32, Span)>,
}

#[derive(Clone, Default, Debug)]
pub struct Module {
    pub funcs: Vec<Func>,
    /// Static data, such as string literals.
    pub data: Vec<Vec<u8>>,
}

impl Module {
    /// Adds static data and gives its number (equal data is shared).
    pub fn add_data(&mut self, bytes: &[u8]) -> u32 {
        if let Some(i) = self.data.iter().position(|d| d == bytes) {
            return i as u32;
        }
        self.data.push(bytes.to_vec());
        self.data.len() as u32 - 1
    }

    /// Adds `size` bytes of zeroed static data of its own (for variables that live for the whole program).
    pub fn add_zeroed_data(&mut self, size: usize) -> u32 {
        self.data.push(vec![0; size]);
        self.data.len() as u32 - 1
    }

    pub fn add(&mut self, f: Func) -> FuncId {
        self.funcs.push(f);
        FuncId(self.funcs.len() as u32 - 1)
    }
}

impl Func {
    /// A function with its entry block (block 0) holding the parameters.
    pub fn new(name: &str, params: &[IrTy], ret: IrTy) -> Func {
        let mut f = Func {
            name: name.to_string(),
            params: params.to_vec(),
            ret,
            values: Vec::new(),
            blocks: Vec::new(),
            insts: Vec::new(),
            checkfree: None,
        };
        let entry = f.new_block(params);
        debug_assert_eq!(entry, BlockId(0));
        f
    }

    pub fn entry(&self) -> BlockId {
        BlockId(0)
    }

    pub fn new_value(&mut self, ty: IrTy) -> Value {
        self.values.push(ty);
        Value(self.values.len() as u32 - 1)
    }

    pub fn ty(&self, v: Value) -> IrTy {
        self.values[v.0 as usize]
    }

    pub fn new_block(&mut self, params: &[IrTy]) -> BlockId {
        let ps = params.iter().map(|t| self.new_value(*t)).collect();
        self.blocks.push(Block {
            params: ps,
            ..Block::default()
        });
        BlockId(self.blocks.len() as u32 - 1)
    }

    pub fn block_params(&self, b: BlockId) -> &[Value] {
        &self.blocks[b.0 as usize].params
    }

    /// Adds an instruction to the end of a block. `ty` is the type of the result, `None` for
    /// instructions that make no value.
    pub fn emit(&mut self, b: BlockId, op: Op, ty: Option<IrTy>) -> Option<Value> {
        self.emit_at(b, op, ty, Span::new(0, 0))
    }

    pub fn emit_at(&mut self, b: BlockId, op: Op, ty: Option<IrTy>, span: Span) -> Option<Value> {
        let result = ty.map(|t| self.new_value(t));
        self.insts.push(Inst { result, op, span });
        let id = self.insts.len() - 1;
        self.blocks[b.0 as usize].insts.push(id);
        result
    }

    pub fn terminate(&mut self, b: BlockId, t: Term) {
        self.blocks[b.0 as usize].term = Some(t);
    }

    /// Removes the blocks that cannot be reached from the entry and renumbers the rest.
    pub fn compact(&mut self) {
        let n = self.blocks.len();
        let mut seen = vec![false; n];
        let mut stack = vec![0usize];
        seen[0] = true;
        while let Some(b) = stack.pop() {
            for s in self.successors(BlockId(b as u32)) {
                let s = s.0 as usize;
                if s < n && !seen[s] {
                    seen[s] = true;
                    stack.push(s);
                }
            }
        }
        if seen.iter().all(|x| *x) {
            return;
        }
        let mut map = vec![u32::MAX; n];
        let mut next = 0u32;
        for (i, keep) in seen.iter().enumerate() {
            if *keep {
                map[i] = next;
                next += 1;
            }
        }
        let old = std::mem::take(&mut self.blocks);
        for (i, mut b) in old.into_iter().enumerate() {
            if !seen[i] {
                continue;
            }
            let fix = |t: &mut Target| t.block = BlockId(map[t.block.0 as usize]);
            match &mut b.term {
                Some(Term::Jump(t)) => fix(t),
                Some(Term::Branch(_, a, c)) => {
                    fix(a);
                    fix(c);
                }
                _ => {}
            }
            self.blocks.push(b);
        }
    }

    /// The blocks a block can go to.
    pub fn successors(&self, b: BlockId) -> Vec<BlockId> {
        match &self.blocks[b.0 as usize].term {
            Some(Term::Jump(t)) => vec![t.block],
            Some(Term::Branch(_, a, c)) => vec![a.block, c.block],
            _ => Vec::new(),
        }
    }
}
